# Unified Session Model Design

> 统一消息模型设计：对齐 OpenCode 记忆管理架构

**日期**: 2026-01-24
**状态**: 已批准
**参考**: OpenCode (`/Users/zouguojun/Workspace/opencode`)

---

## 1. 背景与问题

### 1.1 当前架构缺陷

Aether 存在**两套并行的状态模型**，导致已实现的功能无法生效：

```
┌─────────────────────────────────────────────────────────────┐
│  agent_loop/state.rs          │  components/types.rs        │
├───────────────────────────────┼─────────────────────────────┤
│  LoopState                    │  ExecutionSession           │
│  ├─ steps: Vec<LoopStep>      │  ├─ parts: Vec<SessionPart> │
│  ├─ history_summary           │  ├─ total_tokens            │
│  └─ compressed_until_step     │  └─ model                   │
│                               │                             │
│  (agent_loop 内部使用)          │  (SessionCompactor 使用)    │
│  (消息构建基于此)               │  (过滤逻辑基于此)            │
└───────────────────────────────┴─────────────────────────────┘
```

**问题**：
- `filter_compacted()` 已实现但 agent_loop 未调用
- `is_overflow()` 仅在 `compact()` 内部使用，非实时检测
- 系统提醒注入机制缺失
- 缓存优化未实现

### 1.2 目标能力（对齐 OpenCode）

| 能力 | 说明 |
|------|------|
| 细粒度消息过滤 | `filterCompacted()` 跳过已压缩历史 |
| 实时溢出检测 | 每次 LLM 响应后检查，溢出则触发压缩 |
| 系统提醒注入 | `<system-reminder>` 在多步骤中保持上下文 |
| 缓存优化 | 两部分系统提示，最大化 Anthropic 缓存命中 |

---

## 2. 设计方案

### 2.1 核心数据结构统一

废弃 `LoopState` + `LoopStep`，统一使用 `ExecutionSession` + `SessionPart`：

```rust
// 增强 ExecutionSession，吸收 LoopState 的字段
pub struct ExecutionSession {
    // 现有字段保留
    pub id: String,
    pub parts: Vec<SessionPart>,
    pub total_tokens: u64,
    pub model: String,

    // 从 LoopState 迁移
    pub original_request: String,        // 新增
    pub context: RequestContext,         // 新增
    pub started_at: i64,                 // 新增

    // 压缩相关（对齐 OpenCode）
    pub last_compaction_index: usize,    // 替代 compressed_until_step
    pub needs_compaction: bool,          // 新增：标记需要压缩
}
```

### 2.2 SessionPart 增强

```rust
pub enum SessionPart {
    // 现有类型保留
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    CompactionMarker(CompactionMarker),

    // 新增：系统提醒（对齐 OpenCode）
    SystemReminder(SystemReminderPart),
}

pub struct SystemReminderPart {
    pub content: String,
    pub reminder_type: ReminderType,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReminderType {
    ContinueTask,
    MaxStepsWarning { current: usize, max: usize },
    TokenLimitWarning { usage_percent: u8 },
    PlanMode { plan_file: String },
    Custom { source: String, content: String },
}
```

---

## 3. 消息构建流水线

### 3.1 MessageBuilder 模块

```rust
// core/src/agent_loop/message_builder.rs

pub struct MessageBuilder {
    compactor: Arc<SessionCompactor>,
    config: MessageBuilderConfig,
}

impl MessageBuilder {
    /// 核心方法：从 ExecutionSession 构建 LLM 消息
    pub fn build_messages(&self, session: &ExecutionSession) -> Vec<Message> {
        // 1. 过滤已压缩的历史（对齐 OpenCode filterCompacted）
        let filtered_parts = self.compactor.filter_compacted(session);

        // 2. 转换为 LLM 消息格式
        let messages = self.parts_to_messages(&filtered_parts);

        // 3. 注入系统提醒（多步骤时）
        self.inject_reminders(messages, session)
    }
}
```

### 3.2 Parts → Messages 转换规则

```rust
fn parts_to_messages(&self, parts: &[SessionPart]) -> Vec<Message> {
    let mut messages = Vec::new();

    for part in parts {
        match part {
            // 用户输入 → User Message
            SessionPart::UserInput(p) => {
                messages.push(Message::user(&p.text));
            }

            // AI 响应 → Assistant Message
            SessionPart::AiResponse(p) => {
                messages.push(Message::assistant(&p.content));
            }

            // 工具调用 → Assistant (调用) + User (结果)
            SessionPart::ToolCall(p) if p.status == ToolCallStatus::Completed => {
                messages.push(Message::assistant_tool_call(&p.tool_name, &p.input));
                messages.push(Message::tool_result(&p.id, p.output.as_deref().unwrap_or("")));
            }

            // 中断的工具调用 → 错误提示
            SessionPart::ToolCall(p) if p.status == ToolCallStatus::Pending => {
                messages.push(Message::tool_result(&p.id, "[Tool execution was interrupted]"));
            }

            // 摘要 → 特殊 User Message
            SessionPart::Summary(p) => {
                messages.push(Message::user("What did we do so far?"));
                messages.push(Message::assistant(&p.content));
            }

            _ => {}
        }
    }

    messages
}
```

### 3.3 系统提醒注入

```rust
fn inject_reminders(
    &self,
    mut messages: Vec<Message>,
    session: &ExecutionSession
) -> Vec<Message> {
    let iteration = session.iteration_count;

    // 仅在第 2 步之后注入
    if iteration <= 1 {
        return messages;
    }

    // 找到最后一个用户消息并包装
    if let Some(last_user_idx) = messages.iter().rposition(|m| m.role == Role::User) {
        let original_text = &messages[last_user_idx].content;

        let reminded_text = format!(
            "<system-reminder>\n\
            The user sent the following message:\n\
            {}\n\
            Please address this message and continue with your tasks.\n\
            </system-reminder>",
            original_text
        );

        messages[last_user_idx].content = reminded_text;
    }

    // 注入限制警告
    self.inject_limit_warnings(&mut messages, session);

    messages
}
```

---

## 4. 实时溢出检测与自动压缩

### 4.1 主循环集成

```rust
impl AgentLoop {
    async fn run_loop(&self, session: &mut ExecutionSession) -> LoopResult {
        loop {
            // ===== 实时溢出检测（每次迭代前）=====
            if self.should_compact(session) {
                self.perform_compaction(session).await?;
            }

            // ===== 构建消息 =====
            let messages = self.message_builder.build_messages(session);

            // ===== 调用 LLM =====
            let response = self.thinker.think_with_messages(messages, &tools).await?;

            // ===== 记录响应 =====
            self.record_response(session, &response);

            // ===== 响应后检查溢出 =====
            if self.is_overflow(session) {
                session.needs_compaction = true;
            }
        }
    }

    fn is_overflow(&self, session: &ExecutionSession) -> bool {
        let model_limit = self.get_model_limit(&session.model);
        let output_reserve = model_limit.max_output.min(32_000);
        let usable = model_limit.context - output_reserve;

        session.total_tokens > usable
    }
}
```

### 4.2 两阶段压缩

```rust
async fn perform_compaction(&self, session: &mut ExecutionSession) -> Result<()> {
    // 阶段 1：修剪老旧工具输出
    let prune_result = self.compactor.prune_old_tool_outputs(session);

    if prune_result.tokens_pruned > PRUNE_MINIMUM {
        return Ok(());
    }

    // 阶段 2：生成摘要
    if self.is_overflow(session) {
        self.compactor.insert_compaction_marker(session, true);
        let summary = self.compactor.generate_summary(session).await?;
        self.compactor.replace_with_summary(session, summary);
    }

    session.needs_compaction = false;
    Ok(())
}
```

---

## 5. 缓存优化策略

### 5.1 两部分系统提示

```rust
impl PromptBuilder {
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        // Part 1: 静态头部（高缓存命中率）
        let header = self.build_static_header();

        // Part 2: 动态部分
        let dynamic = vec![
            self.build_agent_instructions(),
            self.build_tool_index(tools),
            self.build_skill_instructions(),
        ].join("\n");

        vec![
            SystemPromptPart { content: header, cache: true },
            SystemPromptPart { content: dynamic, cache: false },
        ]
    }
}

pub struct SystemPromptPart {
    pub content: String,
    pub cache: bool,
}
```

### 5.2 Token 统计增强

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
}

impl TokenUsage {
    pub fn total_for_overflow(&self) -> u64 {
        self.input + self.cache_read + self.output
    }

    pub fn total_billable(&self) -> f64 {
        self.input as f64 * 1.0
        + self.output as f64 * 1.0
        + self.cache_read as f64 * 0.1
        + self.cache_write as f64 * 1.25
    }
}
```

---

## 6. 迁移计划

### 6.1 三阶段路径

| 阶段 | 内容 | 预估 |
|------|------|------|
| Phase 1 | 桥接层：LoopState ↔ ExecutionSession 同步 | 1-2 天 |
| Phase 2 | 消息构建重构：MessageBuilder + 提醒注入 | 2-3 天 |
| Phase 3 | 废弃 LoopState：直接使用 ExecutionSession | 1-2 天 |

### 6.2 文件变更清单

| 阶段 | 操作 | 文件 |
|------|------|------|
| P1 | 新增 | `core/src/agent_loop/session_sync.rs` |
| P1 | 修改 | `core/src/agent_loop/mod.rs` |
| P2 | 新增 | `core/src/agent_loop/message_builder.rs` |
| P2 | 新增 | `core/src/agent_loop/reminder.rs` |
| P2 | 修改 | `core/src/thinker/prompt_builder.rs` |
| P2 | 修改 | `core/src/thinker/mod.rs` |
| P2 | 修改 | `core/src/components/types.rs` |
| P3 | 删除 | `core/src/agent_loop/state.rs` |
| P3 | 删除 | `core/src/agent_loop/session_sync.rs` |

### 6.3 风险控制

```rust
pub struct AgentLoopConfig {
    pub use_unified_session: bool,      // Phase 1-2
    pub use_message_builder: bool,      // Phase 2
    pub use_realtime_overflow: bool,    // Phase 2
}
```

### 6.4 测试策略

- `test_state_sync_consistency` - 验证同步一致性
- `test_message_builder_equivalence` - 验证消息构建等价
- `test_filter_compacted_integration` - 验证过滤集成
- `test_overflow_triggers_compaction` - 验证溢出触发
- `test_reminder_injection` - 验证提醒注入

---

## 7. 验收标准

- [ ] Phase 1: 现有测试通过，状态同步正常
- [ ] Phase 2: 消息正确构建，提醒正常注入，溢出检测生效
- [ ] Phase 3: 代码简洁，无冗余，全部测试通过

---

## 8. 参考

- OpenCode 源码: `/Users/zouguojun/Workspace/opencode`
- OpenCode 关键文件:
  - `packages/opencode/src/session/message-v2.ts` - 消息模型
  - `packages/opencode/src/session/prompt.ts` - 消息构建
  - `packages/opencode/src/session/compaction.ts` - 压缩逻辑
