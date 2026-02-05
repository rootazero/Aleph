# Multi-Agent 2.0 Architecture Design

> **Status**: Draft
> **Created**: 2026-02-05
> **Author**: Architecture Review
> **Baseline**: OpenClaw subagent architecture

---

## Executive Summary

本设计文档定义了 Aleph Multi-Agent 2.0 架构，旨在超越 OpenClaw 的子代理实现，解决以下核心问题：

1. **生存能力 (Resilience)**: 长时任务跨重启恢复
2. **会话持久化 (Session-as-a-Service)**: 子代理支持追问、微调、恢复
3. **实时感知 (Real-time Perception)**: 主代理可订阅子代理事件流，实时监控与干预
4. **资源调度 (Lane Scheduling)**: 多通道隔离调度，防止任务风暴

---

## 1. Architecture Vision

### 1.1 Core Philosophy

```
从 "函数调用" 升级为 "会话实体"
┌─────────────────────────────────────────────────────────────┐
│  Aleph 1.0: SubAgent = spawn() → execute() → result         │
│  Aleph 2.0: SubAgent = Session + Lifecycle + Perception     │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Three Pillars

| Pillar | Description | vs OpenClaw |
|--------|-------------|-------------|
| **Session Persistence** | 子代理拥有独立会话，完成后保持"待命"可追问 | 超越：支持 context window 复用 |
| **Lifecycle Resilience** | 任务状态持久化到 Memory，跨重启恢复 | 对齐：runs.json → FactsDB |
| **Real-time Perception** | 主代理可订阅子代理事件流，实时感知并干预 | 超越：OpenClaw 只有 announce |

### 1.3 Architecture Layers

```
┌─────────────────────────────────────────────────────────┐
│                   Agent Orchestrator                     │  ← 新增：统一编排层
│   SubAgentRegistry │ LaneScheduler │ PerceptionHub      │
├─────────────────────────────────────────────────────────┤
│                   Session Manager                        │  ← 新增：会话持久化
│   SessionStore │ ContextWindow │ TranscriptArchive       │
├─────────────────────────────────────────────────────────┤
│                   Existing: Dispatcher + Coordinator     │  ← 增强：状态持久化
│   DagScheduler │ ExecutionCoordinator │ ResultCollector │
├─────────────────────────────────────────────────────────┤
│                   Memory Layer                           │  ← 复用：FactsDB
│   subagent_run facts │ session_context facts            │
└─────────────────────────────────────────────────────────┘
```

---

## 2. Core Data Models

### 2.1 SubAgentRun

```rust
/// 子代理运行记录 — 持久化到 FactsDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentRun {
    // === 身份标识 ===
    pub run_id: String,              // UUID，主键
    pub session_key: SessionKey,     // 子代理会话标识
    pub parent_session_key: SessionKey, // 请求者会话标识

    // === 任务定义 ===
    pub task: String,                // 任务描述
    pub agent_type: String,          // 代理类型 (explore, plan, execute, ...)
    pub label: Option<String>,       // 显示标签

    // === 生命周期时间戳 ===
    pub created_at: i64,             // 创建时间 (Unix ms)
    pub started_at: Option<i64>,     // 启动时间
    pub ended_at: Option<i64>,       // 结束时间
    pub archived_at: Option<i64>,    // 归档时间

    // === 状态与结果 ===
    pub status: RunStatus,           // Pending | Running | Completed | Failed | Paused
    pub outcome: Option<RunOutcome>, // 执行结果
    pub error: Option<String>,       // 错误信息

    // === 资源配置 ===
    pub lane: Lane,                  // 调度通道
    pub priority: u8,                // 优先级 (0-255)
    pub max_turns: Option<u32>,      // 最大轮次限制
    pub timeout_ms: Option<u64>,     // 超时时间

    // === 恢复元数据 ===
    pub checkpoint_id: Option<String>, // 最后检查点
    pub retry_count: u32,            // 重试次数
    pub cleanup_policy: CleanupPolicy, // Delete | Keep | Archive
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunStatus {
    Pending,      // 已创建，等待调度
    Running,      // 执行中
    Paused,       // 暂停（可恢复）
    Completed,    // 成功完成
    Failed,       // 执行失败
    Cancelled,    // 被取消
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Lane {
    Main,         // 主通道，高优先级
    Subagent,     // 子代理通道，后台优先级
    Cron,         // 定时任务通道
    Nested,       // 嵌套调用通道
}
```

### 2.2 SessionContext

```rust
/// 会话上下文 — 支持跨轮次复用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub session_key: SessionKey,
    pub context_window: Vec<Message>,   // 对话历史
    pub artifacts: Vec<Artifact>,       // 产出物
    pub tool_history: Vec<ToolRecord>,  // 工具调用记录
    pub last_active_at: i64,
    pub ttl_ms: u64,                    // 生存时间
}
```

---

## 3. SubAgentRegistry

### 3.1 Core Structure

```rust
/// 子代理注册表 — 管理所有运行实例
pub struct SubAgentRegistry {
    // === 内存索引（热数据）===
    runs: RwLock<HashMap<String, SubAgentRun>>,      // run_id → run
    by_session: RwLock<HashMap<SessionKey, String>>, // session_key → run_id
    by_parent: RwLock<HashMap<SessionKey, Vec<String>>>, // parent → [run_ids]

    // === 持久化后端 ===
    memory: Arc<MemorySystem>,  // 复用 FactsDB

    // === 生命周期监听 ===
    event_tx: broadcast::Sender<LifecycleEvent>,

    // === 配置 ===
    config: RegistryConfig,
}

pub struct RegistryConfig {
    pub result_ttl_ms: u64,           // 完成结果保留时间 (默认 1 小时)
    pub session_ttl_ms: u64,          // 会话上下文保留时间 (默认 24 小时)
    pub max_concurrent_per_lane: HashMap<Lane, usize>, // 各通道并发限制
    pub archive_sweep_interval_ms: u64, // 归档扫描间隔
}
```

### 3.2 Lifecycle State Machine

```
                    ┌─────────────┐
                    │   Pending   │ ← register()
                    └──────┬──────┘
                           │ schedule()
                    ┌──────▼──────┐
              ┌─────│   Running   │─────┐
              │     └──────┬──────┘     │
        pause()│           │            │ cancel()
              │     ┌──────▼──────┐     │
              └────►│   Paused    │     │
                    └──────┬──────┘     │
                           │ resume()   │
                    ┌──────▼──────┐     │
                    │   Running   │◄────┘ (retry)
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
       ┌──────▼──────┐ ┌───▼────┐ ┌─────▼─────┐
       │  Completed  │ │ Failed │ │ Cancelled │
       └──────┬──────┘ └───┬────┘ └─────┬─────┘
              │            │            │
              └────────────┼────────────┘
                           │ archive()
                    ┌──────▼──────┐
                    │  Archived   │ → 从内存移除，保留在 FactsDB
                    └─────────────┘
```

### 3.3 Persistence Triggers

```rust
impl SubAgentRegistry {
    /// 注册新运行 — 立即持久化
    pub async fn register(&self, run: SubAgentRun) -> Result<String>;

    /// 状态转换 — 每次转换都持久化
    pub async fn transition(&self, run_id: &str, new_status: RunStatus) -> Result<()>;

    /// 启动时恢复 — 从 FactsDB 重建内存索引
    pub async fn restore_on_startup(&self) -> Result<usize>;
}
```

---

## 4. LaneScheduler

### 4.1 Core Structure

```rust
/// Lane 调度器 — 隔离不同优先级的任务流
pub struct LaneScheduler {
    lanes: HashMap<Lane, LaneState>,
    global_semaphore: Arc<Semaphore>,  // 全局并发上限
    config: LaneConfig,
}

pub struct LaneState {
    queue: VecDeque<String>,           // 等待调度的 run_ids
    running: HashSet<String>,          // 正在执行的 run_ids
    semaphore: Arc<Semaphore>,         // 本通道并发信号量
    priority_boost: i8,                // 优先级加成 (-10 到 +10)
}

pub struct LaneQuota {
    pub max_concurrent: usize,          // 并发上限
    pub token_budget_per_min: u64,      // 每分钟 token 配额 (0 = 无限制)
    pub priority: i8,                   // 基础优先级
}
```

### 4.2 Default Quotas

| Lane | max_concurrent | token_budget/min | priority |
|------|----------------|------------------|----------|
| Main | 2 | unlimited | 10 (highest) |
| Subagent | 8 | 500,000 | 5 |
| Cron | 2 | 100,000 | 0 (lowest) |
| Nested | 4 | 200,000 | 8 |

### 4.3 Key Features

- **Recursion Depth Limit**: 最大嵌套深度 5 层，防止任务风暴
- **Anti-Starvation**: 等待超过 30 秒的任务自动提升优先级
- **Token Budget**: 可选的 token 消耗配额控制

---

## 5. PerceptionHub

### 5.1 Core Structure

```rust
/// 感知中枢 — 主代理实时订阅子代理事件流
pub struct PerceptionHub {
    subscriptions: RwLock<HashMap<SessionKey, Vec<Subscription>>>,
    event_bus: broadcast::Sender<PerceptionEvent>,
    shadow_artifacts: RwLock<HashMap<String, Vec<Artifact>>>,
}

#[derive(Debug, Clone)]
pub enum SubscriptionFilter {
    All,                             // 所有事件
    ToolCalls,                       // 仅工具调用
    Artifacts,                       // 仅产出物
    Progress,                        // 仅进度更新
    Terminal,                        // 仅终态（完成/失败）
    Custom(Vec<EventType>),          // 自定义组合
}
```

### 5.2 Event Types

```rust
#[derive(Debug, Clone, Serialize)]
pub enum PerceptionEvent {
    // === 生命周期事件 ===
    RunStarted { run_id, task, agent_type },
    RunCompleted { run_id, outcome, duration_ms },
    RunFailed { run_id, error, recoverable },

    // === 工具调用事件（实时流）===
    ToolCallStarted { run_id, call_id, tool_name, arguments_preview },
    ToolCallProgress { run_id, call_id, progress, status_text },
    ToolCallCompleted { run_id, call_id, output_preview, duration_ms },

    // === Artifact 事件 ===
    ArtifactCreated { run_id, artifact },

    // === 思考过程事件（可选）===
    ThinkingUpdate { run_id, thinking_preview },
}
```

### 5.3 Intervention Actions

```rust
#[derive(Debug, Clone)]
pub enum InterventionAction {
    Pause { reason: String },
    Cancel { reason: String },
    Inject { message: String },
    AdjustQuota { new_priority: Option<u8>, new_timeout: Option<u64> },
}
```

### 5.4 vs OpenClaw

```
┌─────────────────────────────────────────────────────────────────┐
│  OpenClaw: Announce 模式（事后通告）                             │
│                                                                  │
│  SubAgent ──完成──► Registry ──announce──► Main Agent            │
│                                                                  │
│  缺陷：主代理无法知道子代理正在做什么，只能等待最终结果              │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  Aleph 2.0: Perception 模式（实时感知）                          │
│                                                                  │
│  SubAgent ──stream──► PerceptionHub ──broadcast──► Main Agent    │
│      │                      │                           │        │
│      ▼                      ▼                           ▼        │
│  [每个工具调用]         [实时广播]              [可干预/暂停/取消] │
│                                                                  │
│  优势：主代理实时掌握子代理进度，发现跑偏可立即干预                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## 6. SessionManager

### 6.1 Core Structure

```rust
/// 会话管理器 — 子代理会话持久化与复用
pub struct SessionManager {
    sessions: RwLock<HashMap<SessionKey, ManagedSession>>,
    memory: Arc<MemorySystem>,
    config: SessionConfig,
}

pub struct ManagedSession {
    pub key: SessionKey,
    pub context: SessionContext,
    pub state: SessionState,
    pub created_at: i64,
    pub last_active_at: i64,
    pub access_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Active,      // 正在执行
    Idle,        // 空闲待命（可追问）
    Suspended,   // 挂起（已持久化到磁盘）
    Expired,     // 已过期（待清理）
}

pub struct SessionConfig {
    pub idle_timeout_ms: u64,        // 空闲超时 (默认 30 分钟)
    pub suspend_timeout_ms: u64,     // 挂起超时 (默认 24 小时)
    pub max_active_sessions: usize,  // 最大活跃会话数 (默认 50)
    pub max_context_tokens: usize,   // 单会话最大 token (默认 100k)
    pub compress_threshold: f32,     // 压缩阈值 (默认 0.8)
}
```

### 6.2 Session State Machine

```
┌─────────────┐
│   Active    │ ← execute() / resume()
└──────┬──────┘
       │ idle_timeout
       ▼
┌─────────────┐
│    Idle     │ ← 可追问、可恢复、可微调
└──────┬──────┘
       │ suspend_timeout 或 内存压力
       ▼
┌─────────────┐
│  Suspended  │ ← context 已持久化到 FactsDB
└──────┬──────┘
       │ expire_timeout 或 显式删除
       ▼
┌─────────────┐
│   Expired   │ → 清理
└─────────────┘
```

### 6.3 Key Operations

```rust
impl SessionManager {
    /// 创建新会话
    pub async fn create(&self, key: SessionKey, initial_context: Option<SessionContext>) -> Result<()>;

    /// 追问 — 复用已有会话上下文
    pub async fn follow_up(&self, key: &SessionKey, message: String) -> Result<SessionContext>;

    /// 微调 — 修改会话行为而不重新开始
    pub async fn adjust(&self, key: &SessionKey, adjustment: SessionAdjustment) -> Result<()>;

    /// 会话完成 — 转为空闲状态
    pub async fn mark_idle(&self, key: &SessionKey) -> Result<()>;
}

#[derive(Debug, Clone)]
pub enum SessionAdjustment {
    InjectSystemPrompt { prompt: String },
    TruncateHistory { keep_last_n: usize },
    ResetToCheckpoint { checkpoint_id: String },
}
```

### 6.4 Memory Management

- **LRU Eviction**: 内存压力时挂起最少使用的 Idle 会话
- **Context Compression**: 超过阈值时自动摘要历史消息
- **Checkpoint**: 支持创建和恢复到指定检查点

---

## 7. AgentOrchestrator

### 7.1 Core Structure

```rust
/// Agent 编排器 — 统一管理所有子系统
pub struct AgentOrchestrator {
    // === 核心子系统 ===
    pub registry: Arc<SubAgentRegistry>,
    pub scheduler: Arc<RwLock<LaneScheduler>>,
    pub perception: Arc<PerceptionHub>,
    pub sessions: Arc<SessionManager>,

    // === 现有组件（增强集成）===
    pub coordinator: Arc<ExecutionCoordinator>,
    pub collector: Arc<ResultCollector>,
    pub dispatcher: Arc<DagScheduler>,

    // === 存储后端 ===
    pub memory: Arc<MemorySystem>,

    // === 运行时 ===
    shutdown_tx: broadcast::Sender<()>,
    background_tasks: JoinSet<()>,
}
```

### 7.2 SpawnHandle API

```rust
/// 子代理句柄 — 提供给主代理的操作接口
pub struct SpawnHandle {
    pub run_id: String,
    pub session_key: SessionKey,
    pub perception_rx: Option<broadcast::Receiver<PerceptionEvent>>,
    orchestrator: Arc<AgentOrchestrator>,
}

impl SpawnHandle {
    /// 同步等待完成
    pub async fn wait(&self, timeout: Duration) -> Result<RunOutcome>;

    /// 追问（复用会话）
    pub async fn follow_up(&self, message: String) -> Result<SpawnHandle>;

    /// 暂停
    pub async fn pause(&self, reason: &str) -> Result<()>;

    /// 取消
    pub async fn cancel(&self, reason: &str) -> Result<()>;

    /// 获取实时 Artifacts
    pub async fn artifacts(&self) -> Vec<Artifact>;
}
```

### 7.3 Background Tasks

| Task | Interval | Purpose |
|------|----------|---------|
| Schedule Loop | 100ms | 调度就绪任务 |
| Anti-Starvation Sweep | 10s | 检测并提升饥饿任务优先级 |
| Session Eviction | 60s | LRU 淘汰过期会话 |
| Archive Sweep | 300s | 清理已归档的运行记录 |

---

## 8. Memory Integration (FactsDB)

### 8.1 Fact Types

| Fact Type | Key Pattern | TTL | Purpose |
|-----------|-------------|-----|---------|
| `subagent:run` | `subagent:run:{run_id}` | Based on cleanup_policy | 运行状态 |
| `subagent:session` | `subagent:session:{session_key}` | 24 hours | 会话上下文 |
| `subagent:checkpoint` | `subagent:checkpoint:{id}` | 72 hours | 检查点快照 |
| `subagent:transcript` | `subagent:transcript:{run_id}` | 7 days | 归档对话记录 |

### 8.2 Storage Compression

```rust
/// 存储优化的会话上下文
#[derive(Debug, Serialize, Deserialize)]
struct StoredSessionContext {
    recent_messages: Vec<Message>,     // 完整消息（最近 N 条）
    history_summary: Option<String>,   // 历史摘要（更早的消息压缩后）
    tool_summaries: Vec<ToolSummary>,  // 工具调用摘要
    artifact_refs: Vec<ArtifactRef>,   // Artifact 引用（只存路径）
    total_messages: usize,
    total_tokens_estimate: usize,
    compression_ratio: f32,
}
```

### 8.3 TTL Strategy

| Status | Cleanup Policy | TTL |
|--------|----------------|-----|
| Pending/Running/Paused | - | None (permanent) |
| Completed/Failed/Cancelled | Delete | 0 (immediate) |
| Completed/Failed/Cancelled | Keep | 1 hour |
| Completed/Failed/Cancelled | Archive | 7 days |

---

## 9. Module Structure

```
core/src/
├── orchestrator/                    # 新增：编排层
│   ├── mod.rs
│   ├── orchestrator.rs              # AgentOrchestrator
│   ├── spawn_handle.rs              # SpawnHandle
│   └── config.rs                    # OrchestratorConfig
│
├── agents/
│   └── sub_agents/
│       ├── mod.rs
│       ├── registry.rs              # SubAgentRegistry (新增)
│       ├── run.rs                   # SubAgentRun (新增)
│       ├── coordinator.rs           # ExecutionCoordinator (增强)
│       └── result_collector.rs      # ResultCollector (增强)
│
├── scheduler/                       # 新增：调度层
│   ├── mod.rs
│   ├── lane.rs                      # Lane, LaneQuota
│   ├── lane_scheduler.rs            # LaneScheduler
│   └── anti_starvation.rs           # 饥饿预防
│
├── perception/                      # 新增：感知层
│   ├── mod.rs
│   ├── hub.rs                       # PerceptionHub
│   ├── events.rs                    # PerceptionEvent
│   ├── subscription.rs              # Subscription, SubscriptionFilter
│   └── intervention.rs              # InterventionAction
│
├── sessions/                        # 新增：会话层
│   ├── mod.rs
│   ├── manager.rs                   # SessionManager
│   ├── context.rs                   # SessionContext
│   ├── compression.rs               # 上下文压缩
│   └── checkpoint.rs                # 检查点管理
│
└── memory/
    ├── facts/
    │   └── subagent.rs              # 新增：SubAgentFactType
    └── ...
```

---

## 10. Implementation Roadmap

| Phase | Duration | Deliverables |
|-------|----------|--------------|
| **Phase 1: Infrastructure** | Week 1-2 | SubAgentRun, FactsDB integration, SubAgentRegistry core |
| **Phase 2: Lane Scheduling** | Week 3-4 | LaneScheduler, recursion limits, anti-starvation |
| **Phase 3: Real-time Perception** | Week 5-6 | PerceptionHub, event streaming, intervention API |
| **Phase 4: Session Persistence** | Week 7-8 | SessionManager, follow-up/adjust API, compression |
| **Phase 5: Orchestrator Integration** | Week 9-10 | AgentOrchestrator, SpawnHandle, Gateway RPC |
| **Phase 6: Documentation & Optimization** | Week 11-12 | Docs, benchmarks, fault injection tests |

---

## 11. Data Flow Diagram

```
Main Agent                Orchestrator              SubAgent                Memory
    │                          │                        │                      │
    │ spawn_subagent(task)     │                        │                      │
    ├─────────────────────────►│                        │                      │
    │                          │ register(run)          │                      │
    │                          ├──────────────────────────────────────────────►│
    │                          │                        │                      │
    │                          │ enqueue(run_id, lane)  │                      │
    │                          ├──────────┐             │                      │
    │                          │          │ schedule    │                      │
    │         SpawnHandle      │◄─────────┘             │                      │
    │◄─────────────────────────│                        │                      │
    │                          │                        │                      │
    │ subscribe(events)        │ transition(Running)    │                      │
    ├─────────────────────────►├──────────────────────────────────────────────►│
    │                          │                        │                      │
    │                          │ execute_run()          │                      │
    │                          ├───────────────────────►│                      │
    │                          │                        │                      │
    │  ◄──── PerceptionEvent ──┤◄── tool_call_started ──┤                      │
    │  ◄──── PerceptionEvent ──┤◄── tool_call_complete ─┤                      │
    │  ◄──── PerceptionEvent ──┤◄── artifact_created ───┤                      │
    │                          │                        │                      │
    │ intervene(Pause)         │                        │                      │
    ├─────────────────────────►│ transition(Paused)     │                      │
    │                          ├────────────────────────┤                      │
    │                          ├──────────────────────────────────────────────►│
    │                          │                        │                      │
    │ (later) resume()         │                        │                      │
    ├─────────────────────────►│ transition(Running)    │                      │
    │                          ├───────────────────────►│                      │
    │                          │                        │                      │
    │                          │◄────── complete ───────┤                      │
    │  ◄──── RunCompleted ─────┤                        │                      │
    │                          │ mark_idle(session)     │                      │
    │                          ├──────────────────────────────────────────────►│
    │                          │                        │                      │
    │ follow_up("clarify...")  │                        │                      │
    ├─────────────────────────►│ reuse session_key      │                      │
    │                          ├───────────────────────►│ (context preserved)  │
    │                          │                        │                      │
```

---

## 12. References

- **OpenClaw subagent-registry**: `/Volumes/TBU4/Workspace/openclaw/src/agents/subagent-registry.ts`
- **OpenClaw subagent-announce**: `/Volumes/TBU4/Workspace/openclaw/src/agents/subagent-announce.ts`
- **Aleph ExecutionCoordinator**: `/Volumes/TBU4/Workspace/Aleph/core/src/agents/sub_agents/coordinator.rs`
- **Aleph DagScheduler**: `/Volumes/TBU4/Workspace/Aleph/core/src/dispatcher/scheduler/dag.rs`
