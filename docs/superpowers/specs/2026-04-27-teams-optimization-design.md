# Teams 模块优化设计文档

> 学习 ClawTeam 经验，融合 Aleph 架构优势，分阶段改进 teams 模块

**日期**: 2026-04-27  
**作者**: Sisyphus + User  
**状态**: Design Complete, Pending Review  

---

## 1. 执行摘要

### 1.1 背景

Aleph 的 teams 模块已有基础功能（团队管理、消息系统、协作会话），但对比 ClawTeam 实现存在明显差距：

| 维度 | Aleph 现状 | ClawTeam | 差距 |
|------|-----------|----------|------|
| 事件系统 | 纯审计日志（EventLogStore） | Event Bus（可订阅、钩子） | 缺少事件驱动能力 |
| 任务管理 | Artifact（无状态流转） | Kanban 看板 + 依赖管理 | 缺少可视化工作流 |
| 工作区隔离 | ❌ 无 | Git worktree per-agent | 多代理代码冲突风险 |
| 运行时注入 | ❌ 无 | RuntimeRouter 主动推送 | 无法动态协调代理 |
| 传输抽象 | SQLite 单进程 | 可插拔（file/p2p） | 不支持跨进程/跨机 |

### 1.2 设计原则

1. **融合而非照搬**: 充分复用 Aleph 现有基础设施（EventBus、Sandbox、GlobalBus），避免重复造轮子
2. **事件驱动**: 将现有审计日志改造为可订阅的事件总线，使 Kanban、通知等功能可响应式扩展
3. **分阶段交付**: Phase 1 修复缺陷 + EventBus，Phase 2 Kanban，Phase 3 运行时能力
4. **向后兼容**: 现有代码通过简单 API 替换即可迁移，不破坏已有功能

### 1.3 成功标准

| 阶段 | 成功标准 |
|------|---------|
| Phase 1 | ① N+1 查询修复 ② 所有 `log_event` 改为 `bus.publish` ③ EventLogStore 作为 EventHandler 自动订阅 |
| Phase 2 | ① Artifact 支持 `status` 字段 ② Kanban 四列查询可用 ③ 自动解阻塞功能工作 |
| Phase 3 | ① 可向指定 agent 发送注入消息 ② Workspace 隔离可用 ③ 僵尸进程检测工作 |

---

## 2. 现状分析

### 2.1 Aleph Teams 模块结构

```
src/teams/
├── mod.rs              # 模块入口，导出公共 API
├── types.rs            # Team, TeamMember, TeamStatus 等核心类型
├── store.rs            # SqliteTeamStore - 团队 CRUD
├── artifacts.rs        # TaskArtifact, ArtifactType - 产物存储
├── events.rs           # EventLogStore trait + SqliteEventLogStore - 审计日志
├── plans.rs            # PlanManager - 计划提交/批准/拒绝工作流
├── context.rs          # InboxContextProvider - 代理上下文注入
├── lifecycle.rs        # 团队生命周期管理
├── messages/           # 消息子系统
│   ├── mod.rs
│   ├── types.rs        # TeamMessage, Recipient, MessageType
│   ├── store.rs        # SqliteMessageStore - 消息持久化（含 N+1 问题）
│   ├── router.rs       # MessageRouter - 路由 + 升级规则
│   └── inbox.rs        # Inbox 助手 - 读取/标记已读
└── sessions/           # 协作会话子系统
    ├── mod.rs
    ├── types.rs        # CollaborativeSession, SessionTurn
    ├── store.rs        # SqliteSessionStore
    └── coordinator.rs  # SessionCoordinator - 会话生命周期
```

### 2.2 关键缺陷

1. **N+1 查询** (`messages/store.rs:read_inbox`, `read_thread`)
   - 现象：每条消息单独查询 recipients/attachments
   - 风险：消息量大时性能急剧下降
   - 修复：使用 JOIN 批量加载

2. **错误处理不健壮** (`events.rs:read_event_row`)
   - 现象：`unwrap_or` 用于 payload 反序列化
   - 风险：脏数据导致 panic
   - 修复：返回 `Result` 并记录错误

3. **事件系统是死存储**
   - 现象：`log_event` 只写入 SQLite，无法被其他模块响应
   - 风险：模块间紧耦合，扩展困难
   - 修复：接入现有 `EventBus` 基础设施

### 2.3 可用基础设施（Aleph 已有）

| 基础设施 | 位置 | 用途 |
|---------|------|------|
| EventBus | `src/event/bus.rs` | 类型安全的事件广播通道 |
| GlobalBus | `src/event/global_bus.rs` | 跨 agent 事件聚合，支持按 agent/session 过滤 |
| EventHandlerRegistry | `src/event/handler.rs` | 管理事件处理器生命周期 |
| Sandbox | `src/sandbox/mod.rs` | OS 级沙箱隔离 |
| WorkspaceSandbox | `src/sandbox/workspace.rs` | 基于目录的 workspace 隔离 |
| ProcessSupervisor | `src/process_supervisor/` | PTY 进程监控 |

---

## 3. 架构设计

### 3.1 总体架构

```
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 3: 运行时层                           │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │TeamRuntimeInjector│ │TeamWorkspaceManager│ │TeamAgentMonitor │ │
│  │  (消息注入)      │  │ (Workspace 隔离) │  │ (进程监控)      │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
│           │                    │                    │            │
│           ▼                    ▼                    ▼            │
│     GlobalBus             Sandbox          ProcessSupervisor     │
│  (按 agent 过滤)      (OS 级隔离)         (PTY 监控)              │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 2: Kanban 层                          │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │  KanbanBoard    │  │KanbanAutoUnblocker│ │  TaskArtifact   │  │
│  │  (状态查询)      │  │ (自动解阻塞)      │  │ (扩展状态字段)  │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
│           │                    │                    │            │
│           ▼                    ▼                    ▼            │
│    SqliteArtifactStore    EventHandler         AlephEvent        │
│                           (事件驱动)                             │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │
┌─────────────────────────────────────────────────────────────────┐
│                      PHASE 1: EventBus 层                        │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │ TeamEventLogger │  │ MessageRouter   │  │SessionCoordinator│ │
│  │ (EventHandler)  │  │ (publish 事件)  │  │ (publish 事件)   │  │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
│           │                    │                    │            │
│           ▼                    ▼                    ▼            │
│     SqliteEventLogStore    EventBus.publish(AlephEvent::Team*)   │
│           ▲                                                      │
│           └──────────────────────────────────────────────────────┘
│                           │
└───────────────────────────┼─────────────────────────────────────┘
                            ▼
                    ┌───────────────┐
                    │   AlephEvent  │
                    │  (新增 Team*  │
                    │   变体)       │
                    └───────────────┘
```

### 3.2 Phase 1: EventBus 接入

#### 3.2.1 新增 AlephEvent 变体

```rust
// src/event/types.rs

pub enum AlephEvent {
    // ... 现有变体 ...
    
    // Team events (新增)
    TeamMessageSent(TeamMessageEvent),
    TeamMessageRead(TeamMessageReadEvent),
    TeamSessionStarted(TeamSessionEvent),
    TeamSessionConcluded(TeamSessionEvent),
    TeamPlanSubmitted(TeamPlanEvent),
    TeamPlanResolved(TeamPlanResolvedEvent),
    TeamMemberAdded(TeamMemberEvent),
    TeamMemberRemoved(TeamMemberEvent),
    TeamTaskUnblocked(TeamTaskUnblockedEvent),  // Phase 2 使用
}

pub struct TeamMessageEvent {
    pub team_id: String,
    pub message_id: String,
    pub from_agent: String,
    pub to_agents: Vec<String>,
    pub subject: String,
    pub timestamp: i64,
}

pub struct TeamSessionEvent {
    pub team_id: String,
    pub session_id: String,
    pub trigger_agent: String,
    pub outcome: Option<SessionOutcome>,  // Concluded 时有值
}

pub struct TeamPlanEvent {
    pub team_id: String,
    pub artifact_id: String,
    pub submitter: String,
    pub leader: String,
    pub approved: bool,  // Resolved 时有意义
}

pub struct TeamMemberEvent {
    pub team_id: String,
    pub agent_id: String,
    pub role: String,
}
```

#### 3.2.2 EventLogStore 改造为 EventHandler

```rust
// src/teams/events.rs

/// 将审计日志功能改造为事件处理器
pub struct TeamEventLogger {
    store: SqliteEventLogStore,
}

#[async_trait]
impl EventHandler for TeamEventLogger {
    fn name(&self) -> &'static str {
        "TeamEventLogger"
    }
    
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamMessageSent,
            EventType::TeamMessageRead,
            EventType::TeamSessionStarted,
            EventType::TeamSessionConcluded,
            EventType::TeamPlanSubmitted,
            EventType::TeamPlanResolved,
            EventType::TeamMemberAdded,
            EventType::TeamMemberRemoved,
        ]
    }
    
    async fn handle(
        &self,
        event: &AlephEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // 将 Team 事件转换为审计日志格式并持久化
        if let Some(team_event) = self.convert_to_team_event(event) {
            let _ = self.store.log_event(team_event).await;
        }
        Ok(vec![])
    }
}
```

#### 3.2.3 调用点迁移

```rust
// MessageRouter::send (修改前)
let _ = self.event_store.log_event(NewTeamEvent { ... }).await;

// MessageRouter::send (修改后)
ctx.bus.publish(AlephEvent::TeamMessageSent(TeamMessageEvent { ... })).await;

// SessionCoordinator::start_session (修改后)
ctx.bus.publish(AlephEvent::TeamSessionStarted(TeamSessionEvent { ... })).await;

// PlanManager::submit_plan (修改后)
ctx.bus.publish(AlephEvent::TeamPlanSubmitted(TeamPlanEvent { ... })).await;
```

#### 3.2.4 N+1 查询修复

```rust
// messages/store.rs

// 修复前: read_inbox 每条消息单独查询 recipients
// 修复后: 使用 JOIN 批量加载

pub async fn read_inbox_optimized(
    &self,
    agent_id: &str,
    team_id: &str,
    msg_type: Option<&MessageType>,
) -> Result<Vec<TeamMessage>> {
    let sql = r#"
        SELECT 
            m.id, m.team_id, m.from_agent, m.msg_type, m.subject, m.content,
            m.thread_id, m.reply_to, m.attachments, m.created_at,
            GROUP_CONCAT(r.agent_id || ':' || r.role) as recipients
        FROM team_messages m
        LEFT JOIN team_message_recipients r ON m.id = r.message_id
        WHERE m.team_id = ?1
          AND (r.agent_id = ?2 OR m.from_agent = ?2)
          AND (?3 IS NULL OR m.msg_type = ?3)
        GROUP BY m.id
        ORDER BY m.created_at DESC
    "#;
    // ... 执行并解析 recipients
}
```

---

### 3.3 Phase 2: Kanban + 任务依赖

#### 3.3.1 扩展 TaskArtifact

```rust
// src/teams/artifacts.rs

pub struct TaskArtifact {
    pub id: String,
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,
    pub status: TaskStatus,           // 新增
    pub blocked_by: Vec<String>,      // 新增：阻塞该任务的 artifact IDs
    pub assignee: Option<String>,     // 新增
    pub priority: i32,                // 新增
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Completed => "completed",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Failed => "failed",
        }
    }
}
```

#### 3.3.2 数据库迁移

```sql
-- migrations/2026-04-27-kanban.sql

-- 扩展 task_artifacts 表
ALTER TABLE task_artifacts ADD COLUMN status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE task_artifacts ADD COLUMN blocked_by TEXT NOT NULL DEFAULT '[]';
ALTER TABLE task_artifacts ADD COLUMN assignee TEXT;
ALTER TABLE task_artifacts ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_artifacts ADD COLUMN started_at TEXT;
ALTER TABLE task_artifacts ADD COLUMN completed_at TEXT;

-- 为 status 查询创建索引
CREATE INDEX idx_task_artifacts_status ON task_artifacts(task_id, status);
CREATE INDEX idx_task_artifacts_assignee ON task_artifacts(assignee);
```

#### 3.3.3 KanbanBoard 接口

```rust
// src/teams/kanban/mod.rs

#[async_trait]
pub trait KanbanBoard: Send + Sync {
    /// 获取团队任务看板
    async fn get_board(&self, team_id: &str) -> Result<KanbanColumns>;
    
    /// 移动任务状态
    async fn move_task(
        &self,
        artifact_id: &str,
        new_status: TaskStatus,
    ) -> Result<TaskArtifact>;
    
    /// 完成任务（触发依赖解阻塞）
    async fn complete_task(&self, artifact_id: &str) -> Result<Vec<TaskArtifact>>;
    
    /// 添加任务依赖
    async fn add_dependency(
        &self,
        artifact_id: &str,
        depends_on: &str,
    ) -> Result<()>;
}

pub struct KanbanColumns {
    pub pending: Vec<TaskArtifact>,
    pub in_progress: Vec<TaskArtifact>,
    pub completed: Vec<TaskArtifact>,
    pub blocked: Vec<TaskArtifact>,
}

pub struct SqliteKanbanBoard {
    artifact_store: Arc<dyn ArtifactStore>,
    conn: Arc<Mutex<Connection>>,
}

#[async_trait]
impl KanbanBoard for SqliteKanbanBoard {
    async fn get_board(&self, team_id: &str) -> Result<KanbanColumns> {
        let all = self.load_team_artifacts(team_id).await?;
        Ok(KanbanColumns {
            pending: all.iter().filter(|a| a.status == TaskStatus::Pending).cloned().collect(),
            in_progress: all.iter().filter(|a| a.status == TaskStatus::InProgress).cloned().collect(),
            completed: all.iter().filter(|a| a.status == TaskStatus::Completed).cloned().collect(),
            blocked: all.iter().filter(|a| a.status == TaskStatus::Blocked).cloned().collect(),
        })
    }
    
    async fn complete_task(&self, artifact_id: &str) -> Result<Vec<TaskArtifact>> {
        // 1. 更新任务状态为 Completed
        self.update_status(artifact_id, TaskStatus::Completed).await?;
        
        // 2. 查询所有被该任务阻塞的任务
        let blocked = self.find_blocked_tasks(artifact_id).await?;
        
        // 3. 检查这些任务的依赖是否全部完成
        let mut unblocked = vec![];
        for task in blocked {
            if self.all_dependencies_completed(&task).await? {
                self.update_status(&task.id, TaskStatus::Pending).await?;
                unblocked.push(task);
            }
        }
        
        Ok(unblocked)
    }
}
```

#### 3.3.4 自动解阻塞事件处理器

```rust
// src/teams/kanban/unblocker.rs

pub struct KanbanAutoUnblocker {
    kanban: Arc<dyn KanbanBoard>,
    msg_router: Arc<MessageRouter>,
}

#[async_trait]
impl EventHandler for KanbanAutoUnblocker {
    fn name(&self) -> &'static str {
        "KanbanAutoUnblocker"
    }
    
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::TeamPlanResolved,
            EventType::TeamArtifactSubmitted,
        ]
    }
    
    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        let artifact_id = match event {
            AlephEvent::TeamPlanResolved(e) if e.approved => &e.artifact_id,
            AlephEvent::TeamArtifactSubmitted(e) => &e.artifact_id,
            _ => return Ok(vec![]),
        };
        
        // 完成任务并获取解阻塞的任务列表
        let unblocked = self.kanban.complete_task(artifact_id).await
            .map_err(|e| HandlerError::Internal(e.to_string()))?;
        
        // 为每个解阻塞的任务发送通知
        for task in unblocked {
            self.msg_router.send(SendRequest {
                team_id: task.team_id.clone(),
                from_agent: "system".to_string(),
                to: vec![task.assignee.unwrap_or_else(|| task.agent_id.clone())],
                cc: vec![],
                msg_type: MessageType::TaskUnblocked,
                subject: format!("Task unblocked: {}", task.title),
                content: format!(
                    "Dependency completed. You can now start working on this task."
                ),
                reply_to: None,
                attachments: vec![],
            }).await.ok();
            
            // 发布解阻塞事件
            ctx.bus.publish(AlephEvent::TeamTaskUnblocked(
                TeamTaskUnblockedEvent {
                    team_id: task.team_id,
                    task_id: task.id,
                    unblocked_by: artifact_id.clone(),
                }
            )).await;
        }
        
        Ok(vec![])
    }
}
```

#### 3.3.5 新增 MessageType

```rust
// src/teams/messages/types.rs

pub enum MessageType {
    // 现有类型...
    Message,
    PlanApprovalRequest,
    PlanApproved,
    PlanRejected,
    SystemNotification,
    
    // 新增类型
    TaskAssigned,
    TaskStatusChanged,
    TaskUnblocked,
    DependencyAdded,
}
```

---

### 3.4 Phase 3: 运行时注入 + Workspace 隔离

#### 3.4.1 TeamRuntimeInjector

利用 `GlobalBus` 的按 agent 过滤能力：

```rust
// src/teams/runtime/injector.rs

pub struct TeamRuntimeInjector {
    global_bus: &'static GlobalBus,
}

impl TeamRuntimeInjector {
    pub fn new() -> Self {
        Self {
            global_bus: GlobalBus::global(),
        }
    }
    
    /// 向特定 agent 注入消息/指令
    pub async fn inject_to_agent(
        &self,
        agent_id: &str,
        injection: AgentInjection,
    ) -> Result<()> {
        let event = AlephEvent::TeamInjection(TeamInjectionEvent {
            target_agent: agent_id.to_string(),
            injection,
            timestamp: Utc::now().timestamp_millis(),
        });
        
        // GlobalBus 自动路由到订阅了该 agent 的 EventBus
        self.global_bus.broadcast(agent_id, "", event).await;
        Ok(())
    }
    
    /// 广播给团队所有成员
    pub async fn broadcast_to_team(
        &self,
        team_id: &str,
        member_ids: &[String],
        injection: AgentInjection,
    ) -> Vec<Result<()>> {
        let mut results = vec![];
        for agent_id in member_ids {
            results.push(self.inject_to_agent(agent_id, injection.clone()).await);
        }
        results
    }
}

pub enum AgentInjection {
    NewTask {
        artifact_id: String,
        title: String,
        description: String,
        priority: i32,
    },
    StatusQuery,  // 查询 agent 当前状态
    Interrupt {
        reason: String,
        save_state: bool,  // 是否保存当前状态
    },
    ContextUpdate {
        key: String,
        value: serde_json::Value,
    },
}

// Agent 侧接收注入的事件定义
pub struct TeamInjectionEvent {
    pub target_agent: String,
    pub injection: AgentInjection,
    pub timestamp: i64,
}
```

#### 3.4.2 TeamWorkspaceManager

利用已有 `WorkspaceSandbox`：

```rust
// src/teams/runtime/workspace.rs

pub struct TeamWorkspaceManager {
    base_path: PathBuf,
    sandbox_factory: Arc<dyn SandboxFactory>,
}

impl TeamWorkspaceManager {
    pub fn new(base_path: impl AsRef<Path>) -> Self {
        Self {
            base_path: base_path.as_ref().to_path_buf(),
            sandbox_factory: Arc::new(WorkspaceSandboxFactory::new()),
        }
    }
    
    /// 为 team member 创建隔离 workspace
    pub async fn create_workspace(
        &self,
        team_id: &str,
        agent_id: &str,
    ) -> Result<TeamWorkspace> {
        let workspace_path = self.base_path
            .join("teams")
            .join(team_id)
            .join("workspaces")
            .join(agent_id);
        
        fs::create_dir_all(&workspace_path).await?;
        
        // 使用已有 Sandbox 创建隔离环境
        let sandbox = self.sandbox_factory.create(SandboxConfig {
            workspace_dir: workspace_path.clone(),
            fs_policy: FsPolicy::restricted_to(&workspace_path),
            network_policy: NetworkPolicy::default(),
            process_policy: ProcessPolicy::default(),
        })?;
        
        Ok(TeamWorkspace {
            team_id: team_id.to_string(),
            agent_id: agent_id.to_string(),
            path: workspace_path,
            sandbox,
            created_at: Utc::now(),
        })
    }
    
    /// 创建 checkpoint（用于中断恢复）
    pub async fn checkpoint(&self, workspace: &TeamWorkspace) -> Result<Checkpoint> {
        let checkpoint_id = Uuid::new_v4().to_string();
        let checkpoint_path = workspace.path.join(".checkpoints").join(&checkpoint_id);
        
        fs::create_dir_all(&checkpoint_path).await?;
        
        // 复制当前 workspace 状态到 checkpoint
        self.copy_workspace_state(&workspace.path, &checkpoint_path).await?;
        
        Ok(Checkpoint {
            id: checkpoint_id,
            workspace_path: workspace.path.clone(),
            checkpoint_path,
            created_at: Utc::now(),
        })
    }
    
    /// 从 checkpoint 恢复
    pub async fn restore(&self, checkpoint: &Checkpoint) -> Result<()> {
        self.copy_workspace_state(&checkpoint.checkpoint_path, &checkpoint.workspace_path).await
    }
    
    /// 清理 workspace
    pub async fn cleanup(&self, workspace: TeamWorkspace) -> Result<()> {
        workspace.sandbox.destroy().await?;
        fs::remove_dir_all(&workspace.path).await?;
        Ok(())
    }
}

pub struct TeamWorkspace {
    pub team_id: String,
    pub agent_id: String,
    pub path: PathBuf,
    pub sandbox: Arc<dyn Sandbox>,
    pub created_at: DateTime<Utc>,
}
```

#### 3.4.3 TeamAgentMonitor

轻量级封装 `process_supervisor`：

```rust
// src/teams/runtime/monitor.rs

pub struct TeamAgentMonitor {
    supervisor: Arc<PtySupervisor>,
    registry: Arc<Mutex<AgentProcessRegistry>>,
}

impl TeamAgentMonitor {
    pub fn new(supervisor: Arc<PtySupervisor>) -> Self {
        Self {
            supervisor,
            registry: Arc::new(Mutex::new(AgentProcessRegistry::default())),
        }
    }
    
    /// 注册 agent 进程
    pub async fn register(&self, agent_id: &str, process_id: &str) {
        let mut reg = self.registry.lock().await;
        reg.insert(agent_id.to_string(), process_id.to_string());
    }
    
    /// 检查 agent 是否存活
    pub async fn is_alive(&self, agent_id: &str) -> bool {
        let reg = self.registry.lock().await;
        let Some(process_id) = reg.get(agent_id) else {
            return false;
        };
        
        self.supervisor.process_status(process_id).await
            .map(|s| s.is_running())
            .unwrap_or(false)
    }
    
    /// 获取僵尸进程列表（已退出但未清理）
    pub async fn list_zombies(&self, team_id: &str) -> Vec<ZombieAgent> {
        let reg = self.registry.lock().await;
        let mut zombies = vec![];
        
        for (agent_id, process_id) in reg.iter() {
            let status = self.supervisor.process_status(process_id).await;
            match status {
                Ok(s) if !s.is_running() => {
                    zombies.push(ZombieAgent {
                        agent_id: agent_id.clone(),
                        process_id: process_id.clone(),
                        exit_code: s.exit_code(),
                        detected_at: Utc::now(),
                    });
                }
                Err(_) => {
                    // 无法获取状态，视为僵尸
                    zombies.push(ZombieAgent {
                        agent_id: agent_id.clone(),
                        process_id: process_id.clone(),
                        exit_code: None,
                        detected_at: Utc::now(),
                    });
                }
                _ => {}
            }
        }
        
        zombies
    }
    
    /// 清理僵尸进程
    pub async fn cleanup_zombies(&self, zombies: &[ZombieAgent]) -> Result<usize> {
        let mut reg = self.registry.lock().await;
        let mut cleaned = 0;
        
        for zombie in zombies {
            if reg.remove(&zombie.agent_id).is_some() {
                self.supervisor.cleanup(&zombie.process_id).await.ok();
                cleaned += 1;
            }
        }
        
        Ok(cleaned)
    }
}

#[derive(Default)]
struct AgentProcessRegistry {
    map: HashMap<String, String>,  // agent_id -> process_id
}
```

---

## 4. 迁移路径

### 4.1 Phase 1 迁移清单

| 文件 | 当前代码 | 新代码 | 备注 |
|-----|---------|-------|------|
| `events.rs` | `EventLogStore` trait | 新增 `TeamEventLogger` impl `EventHandler` | 向后兼容 |
| `messages/store.rs` | `read_inbox` N+1 查询 | 使用 JOIN 优化版本 | 性能提升 |
| `messages/router.rs` | 调用 `event_store.log_event()` | `ctx.bus.publish(AlephEvent::TeamMessageSent)` | API 替换 |
| `sessions/coordinator.rs` | 调用 `event_store.log_event()` | `ctx.bus.publish(...)` | API 替换 |
| `plans.rs` | 调用 `event_store.log_event()` | `ctx.bus.publish(...)` | API 替换 |

### 4.2 Phase 2 迁移清单

| 文件 | 操作 | 说明 |
|-----|------|------|
| `artifacts.rs` | 扩展 `TaskArtifact` | 新增 `status`, `blocked_by`, `assignee`, `priority` 字段 |
| `artifacts.rs` | 新增 `TaskStatus` enum | 定义状态流转 |
| 新增 `kanban/mod.rs` | 创建 KanbanBoard trait + SqliteKanbanBoard | 看板功能 |
| 新增 `kanban/unblocker.rs` | 创建 KanbanAutoUnblocker | 自动解阻塞 |
| `messages/types.rs` | 扩展 `MessageType` | 新增 Task 相关消息类型 |

### 4.3 Phase 3 迁移清单

| 文件 | 操作 | 说明 |
|-----|------|------|
| 新增 `runtime/injector.rs` | 创建 TeamRuntimeInjector | 运行时注入 |
| 新增 `runtime/workspace.rs` | 创建 TeamWorkspaceManager | Workspace 隔离 |
| 新增 `runtime/monitor.rs` | 创建 TeamAgentMonitor | 进程监控 |
| `event/types.rs` | 扩展 `AlephEvent` | 新增 TeamInjection 变体 |

---

## 5. 验证策略

### 5.1 测试矩阵

| 阶段 | 单元测试 | 集成测试 | 验收测试 |
|------|---------|---------|---------|
| Phase 1 | EventHandler 订阅/处理 | MessageRouter 事件发布 | 端到端：发送消息 → 事件持久化 |
| Phase 2 | KanbanBoard CRUD | 自动解阻塞流程 | 端到端：完成任务 → 依赖解阻塞 → 通知发送 |
| Phase 3 | RuntimeInjector mock | Workspace 隔离验证 | 端到端：Leader 注入任务 → Agent 接收 → 执行 |

### 5.2 性能基准

| 指标 | 当前 | 目标 | 验证方式 |
|-----|------|------|---------|
| read_inbox (1000 条) | ~500ms (N+1) | <50ms | `cargo bench` |
| 事件发布延迟 | N/A (同步调用) | <10ms | 压力测试 |
| Kanban 查询 | N/A | <20ms (1000 任务) | `cargo bench` |

---

## 6. 风险与缓解

| 风险 | 可能性 | 影响 | 缓解措施 |
|-----|--------|------|---------|
| EventBus 订阅遗漏 | 中 | 高 | 所有现有 `log_event` 调用点做 checklist，集成测试全覆盖 |
| 数据库迁移失败 | 低 | 高 | 提供 rollback SQL，测试环境预演 |
| Workspace 隔离绕过 | 低 | 高 | 复用已有 Sandbox 安全审计，不自己实现隔离 |
| 向后兼容性破坏 | 中 | 中 | 每个 Phase 单独分支，完整测试后合并 |

---

## 7. 附录

### 7.1 术语对照表

| 术语 | Aleph | ClawTeam | 说明 |
|-----|-------|----------|------|
| 事件总线 | EventBus + GlobalBus | EventBus | Aleph 已有更强大的跨 agent 能力 |
| Workspace 隔离 | Sandbox + WorkspaceSandbox | Git worktree | Aleph 使用 OS 级沙箱，更安全 |
| 运行时注入 | GlobalBus 按 agent 过滤 | RuntimeRouter | Aleph 使用内存 channel，延迟更低 |

### 7.2 参考文档

- [Aleph Event System](/Volumes/TBU4/Workspace/Aleph/src/event/mod.rs)
- [Aleph Sandbox](/Volumes/TBU4/Workspace/Aleph/src/sandbox/mod.rs)
- [Aleph Process Supervisor](/Volumes/TBU4/Workspace/Aleph/src/process_supervisor/mod.rs)
- [ClawTeam Event Bus](/Volumes/TBU4/Github/ClawTeam/clawteam/events/bus.py)
- [Aleph Teams 模块现状](/Volumes/TBU4/Workspace/Aleph/src/teams/mod.rs)

---

## 8. 审批记录

| 版本 | 日期 | 变更 | 审批人 |
|-----|------|------|--------|
| 1.0 | 2026-04-27 | 初始设计 | User (pending) |
