# Multi-Agent Resilience & Governance Architecture

> **Status**: Design Complete
> **Date**: 2026-02-05
> **Scope**: ExecutionCoordinator, EventBus, ResourceGovernor, Database Schema

---

## Executive Summary

本设计文档定义了 Aleph 从"脚本执行器"向"Agentic OS"演进的核心架构升级。通过引入**确定性恢复**、**全景感知**、**长效协作**和**资源治理**四大能力，Aleph 将在可靠性和交互深度上全面超越 OpenClaw。

### Design Pillars

| 能力 | 核心机制 | 超越点 |
|------|----------|--------|
| **Resilience** | Shadow Replay + task_traces | 零 Token 消耗的毫秒级任务续接 |
| **Perception** | Skeleton & Pulse Model | 实时流感知 + 轨迹可追溯 |
| **Collaboration** | Session-as-a-Service | 子代理从"工具"变为"虚拟员工" |
| **Stability** | Lanes & Governor | 优先级隔离 + 递归熔断 + 内存换出 |

---

## 1. Resilience: 确定性恢复架构

### 1.1 Recovery Strategy: Risk-Aware Recovery

系统重启后，对于处于 `Running` 状态的未完成任务：

- **低风险任务 (Auto-Resume)**: 只读操作（搜索、分析）自动恢复
- **高风险任务 (Safe-Interruption)**: 涉及 `write_file`、`bash`、`send_message` 的任务标记为 `Interrupted`，等待用户确认

风险等级由现有的 `RiskEvaluator` 判定。

### 1.2 Shadow Replay Mechanism

**核心原则**: 重放阶段不调用 LLM。

```
┌─────────────────────────────────────────────────────────────┐
│                    Shadow Replay Flow                        │
├─────────────────────────────────────────────────────────────┤
│  1. State Reconstruction                                     │
│     └─ Load task_traces from DB → Rebuild ConversationHistory│
│     └─ Zero Token cost, millisecond latency                  │
│                                                              │
│  2. Handover Inference                                       │
│     └─ Resume LLM at last checkpoint                         │
│     └─ Alignment Check: compare next tool_call with history  │
│                                                              │
│  3. Environment Sensitivity                                  │
│     └─ HealthCheck critical dependencies before proceeding   │
│     └─ If env changed → mark as Interrupted                  │
└─────────────────────────────────────────────────────────────┘
```

### 1.3 Graceful Checkpoint

利用 Rust 的 `Drop` trait 和 Signal Handling：

- **SIGTERM**: ExecutionCoordinator 对所有 `Running` 任务执行快速落库
- **Checkpoint 内容**: 序列化 ConversationHistory 到 `checkpoint_snapshot_path`

---

## 2. Perception: 全景感知架构

### 2.1 Skeleton & Pulse Model (分级持久化)

| 事件类型 | 持久化策略 | 示例 |
|----------|-----------|------|
| **Skeleton** | 立即落库 | TaskStarted, ToolCallCompleted, ArtifactCreated |
| **Pulse** | 缓冲区落库 (500ms / 50 tokens) | AiStreamingResponse |
| **Volatile** | 仅内存广播 | HeartbeatStatus, MetricsUpdate |

### 2.2 Dual-Bus Architecture (双总线联动)

**emit_and_record 三部曲**:

```rust
async fn emit_and_record(&self, event: AlephEvent) -> Result<()> {
    // 1. DB Commit (The Truth)
    self.db.insert_event(&event).await?;

    // 2. Bus Broadcast (The Pulse)
    if let Err(e) = self.event_bus.publish(event.clone()).await {
        // 3. Backpressure Handling
        tokio::spawn(async move {
            // Async retry with exponential backoff
        });
    }

    Ok(())
}
```

### 2.3 Observer Protocol (Gap-Fill)

主代理的 Observer 具备自愈能力：

- **实时阶段**: 订阅 EventBus，直接更新 ResultCollector
- **断裂自愈**: 检测 `seq` 跳跃，从数据库补全缺失事件

```sql
-- Gap-Fill Query
SELECT * FROM agent_events
WHERE task_id = ? AND seq BETWEEN ? AND ?
ORDER BY seq ASC;
```

---

## 3. Collaboration: Session-as-a-Service

### 3.1 Subagent Lifecycle

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ Pending  │ ──▶ │ Running  │ ──▶ │  Idle    │ ──▶ │ Swapped  │
└──────────┘     └──────────┘     └──────────┘     └──────────┘
                       │                │                │
                       ▼                ▼                ▼
                  [Executing]      [In Memory]     [In Database]
                                   [Handle Reuse]  [Shadow Replay]
```

### 3.2 Handle Reuse Pattern

```rust
// Main agent holds a SessionHandle
let handle = coordinator.spawn_subagent("explorer", task_a).await?;

// Task A completes, subagent enters Idle state
let result_a = handle.wait().await?;

// Main agent reuses the handle for Task B
// Task B inherits Task A's context via Shadow Replay
let result_b = handle.continue_with(task_b).await?;
```

### 3.3 Agent Swapping (Memory Optimization)

借鉴操作系统虚拟内存：

- **Swap Out**: Idle 代理数量超过阈值 → 序列化到数据库 → 释放内存
- **Swap In**: Handle 被调用 → Shadow Replay 重建内存对象

---

## 4. Stability: 资源治理架构

### 4.1 Lane-Based Priority Isolation

```
┌─────────────────────────────────────────────────────────────┐
│                    ResourceGovernor                          │
├─────────────────────────────────────────────────────────────┤
│  MainLane (Reserved: 20% resources)                          │
│  ├─ User Input                                               │
│  ├─ Abort Commands                                           │
│  └─ Status Queries                                           │
│                                                              │
│  SubagentLane (Shared: 80% resources)                        │
│  ├─ Background Tasks                                         │
│  └─ Subagent Execution                                       │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 Recursive Sentry

- **Tracking**: `recursion_depth` 字段在 `agent_tasks` 表
- **Inheritance**: 派生任务继承 `parent_depth + 1`
- **Circuit Breaker**: 超过阈值 (default: 3) 触发 `RecursionLimitExceeded`

### 4.3 Concurrency & Quota Limits

| 资源 | 默认限制 | 配置路径 |
|------|----------|----------|
| Max Running Subagents | 5 | `governor.max_running` |
| Max Idle Subagents (in memory) | 10 | `governor.max_idle` |
| Max Recursion Depth | 3 | `governor.max_depth` |
| Token Budget per Session | 100k | `governor.token_budget` |

---

## 5. Database Schema

### 5.1 agent_tasks Table

```sql
CREATE TABLE IF NOT EXISTS agent_tasks (
    id TEXT PRIMARY KEY,
    parent_session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    task_prompt TEXT NOT NULL,
    status TEXT NOT NULL,  -- Pending, Running, Completed, Failed, Interrupted, Idle, Swapped
    risk_level TEXT NOT NULL,  -- Low, High
    lane TEXT NOT NULL DEFAULT 'subagent',  -- main, subagent

    -- Recovery data
    checkpoint_snapshot_path TEXT,
    last_tool_call_id TEXT,

    -- Governance
    recursion_depth INTEGER DEFAULT 0,
    parent_task_id TEXT,  -- For recursion tracking

    -- Audit
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    metadata_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_tasks_parent ON agent_tasks(parent_session_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON agent_tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_recursion ON agent_tasks(parent_task_id);
```

### 5.2 task_traces Table

```sql
CREATE TABLE IF NOT EXISTS task_traces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,
    role TEXT NOT NULL,  -- assistant, tool
    content_json TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
);

CREATE INDEX IF NOT EXISTS idx_traces_task ON task_traces(task_id, step_index);
```

### 5.3 agent_events Table

```sql
CREATE TABLE IF NOT EXISTS agent_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    is_structural INTEGER DEFAULT 0,
    timestamp INTEGER NOT NULL,
    FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
);

CREATE INDEX IF NOT EXISTS idx_events_task_seq ON agent_events(task_id, seq);
```

### 5.4 subagent_sessions Table

```sql
CREATE TABLE IF NOT EXISTS subagent_sessions (
    id TEXT PRIMARY KEY,
    agent_type TEXT NOT NULL,  -- explorer, coder, researcher, etc.
    status TEXT NOT NULL,  -- Active, Idle, Swapped
    context_path TEXT,  -- Path to serialized context (for swapped agents)

    -- Handle metadata
    parent_session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_active_at INTEGER NOT NULL,

    -- Resource tracking
    total_tokens_used INTEGER DEFAULT 0,
    total_tool_calls INTEGER DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sessions_status ON subagent_sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON subagent_sessions(parent_session_id);
```

---

## 6. Implementation Roadmap

### Phase 1: Database Foundation (Week 1)
- [ ] Add schema migrations for new tables
- [ ] Implement `TaskRepository` for agent_tasks CRUD
- [ ] Implement `TraceRepository` for task_traces
- [ ] Implement `EventRepository` for agent_events

### Phase 2: Resilience Layer (Week 2)
- [ ] Integrate RiskEvaluator with task creation
- [ ] Implement Shadow Replay in ExecutionCoordinator
- [ ] Add SIGTERM handler for Graceful Checkpoint
- [ ] Add recovery logic on system startup

### Phase 3: Perception Layer (Week 3)
- [ ] Implement Skeleton & Pulse event classification
- [ ] Add emit_and_record pattern to EventBus
- [ ] Implement Observer with Gap-Fill logic
- [ ] Integrate with ResultCollector

### Phase 4: Collaboration Layer (Week 4)
- [ ] Implement SessionHandle abstraction
- [ ] Add Idle state management
- [ ] Implement Agent Swapping (serialize/deserialize)
- [ ] Add handle reuse pattern

### Phase 5: Governance Layer (Week 5)
- [ ] Implement ResourceGovernor module
- [ ] Add Lane-based priority queuing
- [ ] Implement Recursive Sentry
- [ ] Add concurrency and quota enforcement

### Phase 6: Integration & Testing (Week 6)
- [ ] End-to-end integration tests
- [ ] Crash recovery tests
- [ ] Performance benchmarks
- [ ] Documentation updates

---

## 7. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Recovery Time | < 500ms | Time from restart to task resumption |
| Event Latency | < 50ms | Time from event emission to Observer receipt |
| Memory per Idle Agent | < 1MB | Serialized context size |
| Main Lane Responsiveness | < 100ms | User command response time under load |
| Max Concurrent Subagents | 50+ | With swapping enabled |

---

## 8. References

- [Aleph Architecture](../ARCHITECTURE.md)
- [Agent System](../AGENT_SYSTEM.md)
- [Memory System](../MEMORY_SYSTEM.md)
- [Multi-Agent 2.0 Design](./2026-02-01-multi-agent-2.0-design.md)

---

*Generated through collaborative brainstorming session on 2026-02-05*
