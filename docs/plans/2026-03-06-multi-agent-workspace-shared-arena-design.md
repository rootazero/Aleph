# Multi-Agent Workspace: SharedArena Design

> 日期: 2026-03-06
> 状态: Approved
> 作者: Claude + User collaborative brainstorming

## 背景

Aleph 当前设计中 Agent 与 Workspace 是 1:1 绑定关系（`workspace_id = agent_id`）。当多个对等 agent 协作处理同一任务时，需要一种机制来共享任务上下文、中间产物和协作记忆，同时保持 1:1 约束不变。

### 范围界定

- **在范围内**: 对等 agent 之间的协作（Peer 对等、Pipeline 流水线）
- **不在范围内**: SubAgent — 子 agent 完全寄生在主 agent 的 workspace 内，其共享由主 agent 全权管理，不参与 SharedArena

## 设计共识

| 决策点 | 结论 |
|--------|------|
| 协作模式 | 通用机制，支撑对等协作 + 流水线所有模式 |
| Workspace 边界 | 保持 1:1 不变，新增共享层（SharedArena） |
| 共享内容 | 任务目标、中间产物、共享记忆、通信消息 |
| 生命周期 | 混合 — 过程数据临时，共享记忆持久沉淀 |
| 冲突处理 | 分区隔离 + 协调者仲裁 |
| 存储实现 | 复用现有基础设施（SQLite / LanceDB / EventBus） |

## 方案选择

评估了三种方案：

- **A. CollaborationToken（轻量标记）**: 改动最小但缺乏显式管理，会退化为散落的 if-else — 否决
- **B. SharedArena（共享竞技场）**: 显式聚合根，DDD 风格，复杂度适中 — **选定**
- **C. VirtualWorkspace（虚拟共享 Workspace）**: 打破 1:1 约束，改动爆炸半径大 — 否决

---

## 核心领域模型

### SharedArena — 聚合根

```rust
/// 协作竞技场 — 对等 agent 协作的显式聚合根
/// SubAgent 不参与，它们完全寄生在主 agent workspace 内
pub struct SharedArena {
    id: ArenaId,
    manifest: ArenaManifest,
    slots: HashMap<AgentId, ArenaSlot>,
    progress: ArenaProgress,
    status: ArenaStatus,
}

impl Entity for SharedArena { type Id = ArenaId; }
impl AggregateRoot for SharedArena {}
```

### ArenaManifest — 协作契约

```rust
pub struct ArenaManifest {
    goal: String,
    strategy: CoordinationStrategy,
    participants: Vec<Participant>,
    created_by: AgentId,
    created_at: DateTime<Utc>,
}

/// 只保留对等级别的协调策略，层级委派由 SubAgent 机制处理
pub enum CoordinationStrategy {
    Peer { coordinator: AgentId },
    Pipeline { stages: Vec<StageSpec> },
}

pub struct StageSpec {
    agent_id: AgentId,
    description: String,
    depends_on: Vec<AgentId>,
}

pub struct Participant {
    agent_id: AgentId,
    role: ParticipantRole,
    permissions: ArenaPermissions,
}

pub enum ParticipantRole {
    Coordinator,    // 可合并产物、仲裁冲突
    Worker,         // 读写自己的 slot，只读他人 slot
    Observer,       // 只读所有 slot
}

pub struct ArenaPermissions {
    can_write_own_slot: bool,
    can_read_other_slots: bool,
    can_write_shared_memory: bool,
    can_merge: bool,                // 仅 Coordinator
}
```

### ArenaSlot — 产出分区（分区隔离的核心）

```rust
pub struct ArenaSlot {
    agent_id: AgentId,
    artifacts: Vec<Artifact>,
    status: SlotStatus,
    updated_at: DateTime<Utc>,
}

pub enum SlotStatus { Idle, Working, Done, Failed }

pub struct Artifact {
    id: ArtifactId,
    kind: ArtifactKind,
    content: ArtifactContent,
    metadata: HashMap<String, Value>,
    created_at: DateTime<Utc>,
}

pub enum ArtifactKind { Text, Code, File, StructuredData }

pub enum ArtifactContent {
    Inline(String),
    Reference(PathBuf),
}
```

### ArenaProgress — 进度追踪

```rust
pub struct ArenaProgress {
    total_steps: usize,
    completed_steps: usize,
    agent_progress: HashMap<AgentId, AgentProgress>,
}

pub struct AgentProgress {
    assigned: Vec<String>,
    completed: Vec<String>,
    current: Option<String>,
}
```

### ArenaStatus — 生命周期

```rust
pub enum ArenaStatus {
    Created,     // 已创建，等待参与者就绪
    Active,      // 协作进行中
    Settling,    // 任务完成，正在沉淀共享记忆
    Archived,    // 过程数据已清理，共享记忆已持久化
}
```

---

## ArenaHandle — Agent 访问接口

Agent 与 Arena 交互的唯一入口，封装权限检查：

```rust
pub struct ArenaHandle {
    arena_id: ArenaId,
    agent_id: AgentId,
    role: ParticipantRole,
    permissions: ArenaPermissions,
    bus: AgentMessageBus,            // 复用现有 swarm event bus
}

impl ArenaHandle {
    // === 产物操作 ===
    pub fn put_artifact(&self, artifact: Artifact) -> Result<ArtifactId>;
    pub fn get_artifact(&self, agent_id: &AgentId, artifact_id: &ArtifactId) -> Result<&Artifact>;
    pub fn list_artifacts(&self, agent_id: &AgentId) -> Result<Vec<&Artifact>>;

    // === 进度操作 ===
    pub fn report_progress(&self, current: Option<String>, completed: Option<String>) -> Result<()>;
    pub fn get_progress(&self) -> &ArenaProgress;

    // === 共享记忆 ===
    pub fn add_shared_fact(&self, fact: SharedFact) -> Result<()>;
    pub fn query_shared_facts(&self, query: &str) -> Result<Vec<SharedFact>>;

    // === 通信（复用 EventBus）===
    pub fn broadcast(&self, event: ArenaEvent) -> Result<()>;
    pub fn send_to(&self, target: &AgentId, event: ArenaEvent) -> Result<()>;

    // === 协调者专属（需 can_merge）===
    pub fn merge_artifacts(&self, sources: Vec<AgentId>) -> Result<Vec<Artifact>>;
    pub fn begin_settling(&self) -> Result<()>;
}
```

### ArenaEvent

```rust
/// Arena 内的事件类型 — 复用 EventBus 传输
pub enum ArenaEvent {
    ArtifactPublished { agent_id: AgentId, artifact_id: ArtifactId },
    ProgressUpdated { agent_id: AgentId, current: String },
    StageCompleted { agent_id: AgentId },
    MergeRequested { coordinator: AgentId },
    ConflictDetected { description: String },
    SettlingStarted,
}
```

### SharedFact

```rust
/// 协作中产生的知识，任务结束后沉淀到 Memory 系统
pub struct SharedFact {
    content: String,
    source_agent: AgentId,
    confidence: f32,
    tags: Vec<String>,
    created_at: DateTime<Utc>,
}
```

---

## 生命周期管理

### 状态机

```
Created ──→ Active ──→ Settling ──→ Archived
   │           │           │
   └── Drop ←──┘           │  (异常时直接归档，SharedFact 仍沉淀)
                            │
                   ┌────────┴────────┐
                   │  沉淀过程        │
                   │  SharedFact    ──→ Memory (LanceDB, 持久)
                   │  Artifacts     ──→ 归档/清理 (临时)
                   │  EventLog      ──→ 清理 (临时)
                   │  Progress      ──→ 清理 (临时)
                   └─────────────────┘
```

### ArenaManager

```rust
pub struct ArenaManager {
    arenas: HashMap<ArenaId, SharedArena>,
    memory_store: Arc<dyn MemoryStore>,
    bus: AgentMessageBus,
}

impl ArenaManager {
    pub fn create_arena(&mut self, manifest: ArenaManifest) -> Result<HashMap<AgentId, ArenaHandle>>;
    pub fn get_handle(&self, arena_id: &ArenaId, agent_id: &AgentId) -> Result<ArenaHandle>;
    pub async fn settle(&mut self, arena_id: &ArenaId) -> Result<SettleReport>;
    pub fn active_arenas_for(&self, agent_id: &AgentId) -> Vec<&SharedArena>;
}

pub struct SettleReport {
    arena_id: ArenaId,
    facts_persisted: usize,
    artifacts_archived: usize,
    events_cleared: usize,
}
```

### Memory 沉淀

- SharedFact 写入 MemoryStore，`namespace: "shared"`，metadata 中标记 `arena_id` 和 `source_agent`
- workspace 归属创建者（`created_by`），其他 agent 通过 `WorkspaceFilter::Multiple` 跨 workspace 检索
- 查询历史协作记忆：通过 `namespace: "shared"` + metadata 筛选

---

## 与现有系统的集成

### 集成点

| 现有系统 | 集成方式 | 改动量 |
|----------|----------|--------|
| **Dispatcher** | 新增 `Collaborative` 路由变体 | 小 |
| **Agent Loop** | RunContext 加 `arena_handles` 字段 | 小 |
| **EventBus** | AgentLoopEvent 新增 `ArenaEvent` 变体 | 小 |
| **ContextInjector** | Arena 状态追加到现有 team_awareness XML | 小 |
| **Memory** | 复用 `namespace: "shared"` + metadata 筛选 | 无改动 |
| **StateDatabase** | 新增 3 张表 | 中 |
| **SubAgent** | 不参与，零改动 | 无 |

### Dispatcher 集成

```rust
// 扩展 TaskRoute 枚举
Collaborative { participants, strategy } => {
    TaskRoute::ArenaExecution {
        manifest: ArenaManifest { goal, strategy, participants, .. },
    }
}
```

### Agent Loop 集成

```rust
pub struct RunContext {
    // ... existing fields ...
    pub arena_handles: Vec<ArenaHandle>,
}
```

Agent Loop 各阶段与 Arena 的交互：
- **Observe**: `ArenaHandle::get_progress()` 感知团队进度
- **Think**: 现有 ContextInjector 将 Arena 状态注入 team_awareness XML
- **Act**: `ArenaHandle::put_artifact()` 发布产物
- **Feedback**: 通过 ArenaEvent 接收其他 agent 的产物通知

### EventBus 集成

ArenaEvent 包装为 AgentLoopEvent，复用现有三层分类：
- `ArtifactPublished` / `StageCompleted` → Tier 1 (Critical)
- 其余 ArenaEvent → Tier 2 (Important)

### SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS arenas (
    id            TEXT PRIMARY KEY,
    goal          TEXT NOT NULL,
    strategy      TEXT NOT NULL,          -- JSON: CoordinationStrategy
    participants  TEXT NOT NULL,          -- JSON: Vec<Participant>
    created_by    TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'created',
    created_at    TEXT NOT NULL,
    settled_at    TEXT,
    settle_report TEXT                    -- JSON: SettleReport
);

CREATE TABLE IF NOT EXISTS arena_slots (
    arena_id      TEXT NOT NULL REFERENCES arenas(id),
    agent_id      TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'idle',
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (arena_id, agent_id)
);

CREATE TABLE IF NOT EXISTS arena_artifacts (
    id            TEXT PRIMARY KEY,
    arena_id      TEXT NOT NULL REFERENCES arenas(id),
    agent_id      TEXT NOT NULL,
    kind          TEXT NOT NULL,
    content       TEXT,
    reference     TEXT,
    metadata      TEXT,                   -- JSON
    created_at    TEXT NOT NULL
);
```

---

## 架构关系图

```
┌─────────────────────────────────────────────────┐
│                  SharedArena (新增)               │
│  ┌───────────┐ ┌───────────┐ ┌───────────┐      │
│  │ Slot (A)  │ │ Slot (B)  │ │ Slot (C)  │      │
│  │ artifacts │ │ artifacts │ │ artifacts │      │
│  └─────┬─────┘ └─────┬─────┘ └─────┬─────┘      │
│        │              │              │            │
│  ┌─────┴──────────────┴──────────────┴─────┐     │
│  │         SharedMemory (沉淀层)            │     │
│  └─────────────────┬───────────────────────┘     │
│                    │                              │
│  ArenaManifest    ArenaProgress                  │
└────────┬───────────┬──────────────────────────────┘
         │           │
    ┌────┴────┐ ┌────┴────┐    ┌──────────────┐
    │  复用    │ │  复用    │    │   不参与      │
    │EventBus │ │Memory   │    │  SubAgent    │
    │(通信)   │ │System   │    │  (寄生在主   │
    │         │ │(沉淀)   │    │   agent内)   │
    └─────────┘ └─────────┘    └──────────────┘
```

## 运行示例

### 场景 1：Peer 对等协作 — "分析一份技术报告"

```
Dispatcher → 创建 Arena (Peer, coordinator: main-agent)
  → researcher: 读报告 → Slot写入 [关键发现, 风险清单]
  → coder:      验证代码 → Slot写入 [代码评估, 复现结果]
  → (并行执行，互不干扰)
  → main-agent: 读取两个 Slot → merge_artifacts() → 综合报告
  → begin_settling() → SharedFact 沉淀 → Arena Archived
```

### 场景 2：Pipeline 流水线 — "翻译并润色一篇文章"

```
Dispatcher → 创建 Arena (Pipeline: translator → polisher)
  → translator: 翻译 → Slot写入 [中文初稿] → StageCompleted
  → polisher:   读取初稿 → 润色 → Slot写入 [润色终稿]
  → begin_settling() → SharedFact 沉淀("术语映射") → Arena Archived
```

## 设计约束

| 约束 | 描述 |
|------|------|
| 1:1 不变 | Agent ↔ Workspace 绑定不动，Arena 是叠加层 |
| SubAgent 不参与 | 子 agent 寄生在主 agent 内，共享由主 agent 负责 |
| 复用优先 | EventBus、Memory、StateDatabase 全部复用 |
| 分区隔离 | 每个 agent 只写自己的 Slot，协调者负责合并 |
| 混合生命周期 | SharedFact 持久沉淀，其余临时数据随 Arena 归档清理 |
| 逻辑抽象层 | Arena 是现有基础设施之上的协调协议，不是新基础设施 |
