# WorldModel + Dispatcher 架构设计

> **Design Date**: 2026-02-04
> **Phase**: 3 (WorldModel) + 4 (Dispatcher)
> **Status**: Approved, Ready for Implementation

## 设计背景

基于已完成的 Phase 1 (Daemon Manager) 和 Phase 2 (Perception Layer)，现在需要设计 Phase 3 (WorldModel) 和 Phase 4 (Dispatcher) 作为一个整体。

**核心目标**：
1. **WorldModel (大脑)**：订阅 DaemonEventBus 的 Raw Events，推理出用户活动和上下文（Derived Events），维护认知记忆
2. **Dispatcher (手脚)**：订阅 Derived Events，根据 PolicyEngine 做决策，调度任务，触发 Agent
3. **闭环设计**：确保 Reconciliation 机制 - 重启后恢复上下文并询问用户

**设计约束**：
- WorldModel 只存储 Dispatcher 真正需要的状态（避免垃圾数据）
- 接口一致性：确保 WorldModel 产出的 Derived Events 是 Dispatcher 可以直接使用的
- 以终为始：从 Dispatcher 的需求反推 WorldModel 的状态模型

**架构决策**（基于用户选择）：
- **推理触发**：混合策略（关键事件立即、高频批量、定期安全网）
- **状态模型**：分层设计（CoreState/EnhancedContext/InferenceCache）
- **策略规则**：混合策略（MVP 硬编码，通过 Policy trait 保留扩展性）
- **对账机制**：分级响应（High/Medium/Low Risk）+ DispatcherMode 状态机

---

## Part 1: 架构概览与数据流

### 1.1 核心数据流

```
┌─────────────────────────────────────────────────────────────────┐
│                       Perception Layer (Phase 2)                 │
│   TimeWatcher │ ProcessWatcher │ FSWatcher │ SystemWatcher      │
└────────────────────────────┬────────────────────────────────────┘
                             │ Raw Events
                             ↓
                   ┌─────────────────────┐
                   │   DaemonEventBus    │ (tokio::broadcast)
                   └─────────────────────┘
                             │
                ┌────────────┴────────────┐
                ↓                          ↓
    ┌───────────────────────┐   ┌───────────────────────┐
    │   WorldModel (Phase 3) │   │  Dispatcher (Phase 4)  │
    │                        │   │                        │
    │  • Subscribe Raw       │   │  • Subscribe Derived   │
    │  • Infer Context       │   │  • Evaluate Policies   │
    │  • Publish Derived     │───→  • Execute Actions    │
    │  • Persist CoreState   │   │  • Handle Reconcile    │
    └───────────────────────┘   └───────────────────────┘
                │                          │
                ↓                          ↓
        CoreState.json            ActionExecutor
        (KB-level)                (System APIs)
```

### 1.2 事件处理策略

WorldModel 采用 **三策略混合**处理 Raw Events：

| 策略 | 触发条件 | 处理延迟 | 适用场景 |
|------|---------|---------|---------|
| **立即处理** | 关键事件（IDE 启动、屏幕休眠） | < 100ms | 活动状态变化 |
| **批量处理** | 高频事件（文件修改） | 5 秒 | 编程语言推理 |
| **定期推理** | 定时触发 | 30 秒 | 安全网兜底 |

### 1.3 职责分离

| 模块 | 职责 | 不做什么 |
|------|------|---------|
| **WorldModel** | 推理上下文、维护状态、发布 DerivedEvent | ❌ 不做决策、不执行动作 |
| **Dispatcher** | 评估 Policy、调度动作、处理 Reconciliation | ❌ 不做推理、不维护长期状态 |

**接口契约**：WorldModel 的 `CoreState` 就是 Dispatcher 的全部输入，保证接口一致性。

---

## Part 2: WorldModel 详细设计

### 2.1 核心职责

WorldModel 是 Aleph 的"认知中枢"，负责：
- **订阅** DaemonEventBus 的 Raw Events
- **推理** 用户活动、任务上下文、环境约束
- **发布** Derived Events 到 Bus
- **维护** CoreState 并持久化关键状态

**关键原则**：WorldModel **只做推理**，不做决策。决策由 Dispatcher 完成。

### 2.2 三层状态模型实现

```rust
// Layer 1: CoreState (KB-level, 必须持久化)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreState {
    /// 当前用户活动类型
    pub activity: ActivityType,

    /// 当前会话 ID（如果在编程会话中）
    pub session_id: Option<String>,

    /// 待处理动作列表（用于 Reconciliation）
    pub pending_actions: Vec<PendingAction>,

    /// 状态最后更新时间
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityType {
    Idle,
    Programming { language: Option<String>, project: Option<String> },
    Meeting { participants: usize },
    Reading,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub action_type: ActionType,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub risk_level: RiskLevel,
}

// Layer 2: EnhancedContext (可选持久化，启动时重建)
pub struct EnhancedContext {
    /// 当前项目根目录
    pub project_root: Option<PathBuf>,

    /// 主导编程语言
    pub dominant_language: Option<String>,

    /// 系统约束（资源负载）
    pub system_constraint: SystemLoad,

    /// 活动时间统计
    pub activity_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct SystemLoad {
    pub cpu_usage: f64,
    pub memory_pressure: MemoryPressure,
    pub battery_level: Option<u8>,
}

#[derive(Debug, Clone)]
pub enum MemoryPressure {
    Normal,
    Warning,
    Critical,
}

// Layer 3: InferenceCache (纯内存，不持久化)
pub struct InferenceCache {
    /// 最近 100 个 Raw Events 的循环缓冲区
    pub recent_events: CircularBuffer<DaemonEvent>,

    /// 不稳定的推理结果（置信度 < 阈值）
    pub unstable_patterns: HashMap<String, ConfidenceScore>,

    /// 高频事件统计窗口
    pub event_counters: HashMap<String, Counter>,
}
```

### 2.3 事件处理循环

```rust
pub struct WorldModel {
    state: Arc<RwLock<CoreState>>,
    context: Arc<RwLock<EnhancedContext>>,
    cache: Arc<Mutex<InferenceCache>>,
    event_bus: Arc<DaemonEventBus>,
    persistence: Arc<StatePersistence>,
}

impl WorldModel {
    pub async fn run(&self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();
        let mut batch_buffer: Vec<DaemonEvent> = Vec::new();
        let mut last_batch_process = Instant::now();

        loop {
            tokio::select! {
                // 策略 1: 关键事件立即处理
                Ok(event) = rx.recv() => {
                    if self.is_key_event(&event) {
                        self.process_immediate(event).await?;
                    } else {
                        batch_buffer.push(event);
                    }
                }

                // 策略 2: 高频事件批量处理（每 5 秒）
                _ = tokio::time::sleep(Duration::from_secs(5)),
                    if !batch_buffer.is_empty() => {
                    self.process_batch(&batch_buffer).await?;
                    batch_buffer.clear();
                    last_batch_process = Instant::now();
                }

                // 策略 3: 定期安全网推理（每 30 秒）
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    self.periodic_inference().await?;
                }
            }
        }
    }

    /// 判断是否为关键事件（需立即处理）
    fn is_key_event(&self, event: &DaemonEvent) -> bool {
        matches!(
            event,
            DaemonEvent::Raw(RawEvent::ProcessEvent {
                event_type: ProcessEventType::Started,
                name,
                ..
            }) if name.contains("Code") || name.contains("Xcode")
            | DaemonEvent::Raw(RawEvent::SystemStateEvent {
                state_type: SystemStateType::DisplaySleep,
                ..
            })
        )
    }
}
```

### 2.4 推理规则示例

```rust
impl WorldModel {
    /// 立即处理关键事件
    async fn process_immediate(&self, event: DaemonEvent) -> Result<()> {
        let mut state = self.state.write().await;
        let mut context = self.context.write().await;

        match event {
            // 规则 1: IDE 启动 → Programming 活动
            DaemonEvent::Raw(RawEvent::ProcessEvent {
                event_type: ProcessEventType::Started,
                name,
                ..
            }) if name.contains("Code") || name.contains("Xcode") => {
                state.activity = ActivityType::Programming {
                    language: None,
                    project: None
                };
                state.last_updated = Utc::now();

                // 发布 Derived Event
                self.event_bus.send(DaemonEvent::Derived(
                    DerivedEvent::ActivityChanged {
                        timestamp: Utc::now(),
                        old_activity: state.activity.clone(),
                        new_activity: ActivityType::Programming {
                            language: None,
                            project: None
                        },
                        confidence: 0.95,
                    }
                ))?;
            }

            // 规则 2: 显示器休眠 → Idle 活动
            DaemonEvent::Raw(RawEvent::SystemStateEvent {
                state_type: SystemStateType::DisplaySleep,
                new_value,
                ..
            }) if new_value.as_bool() == Some(true) => {
                let old_activity = state.activity.clone();
                state.activity = ActivityType::Idle;
                state.last_updated = Utc::now();

                self.event_bus.send(DaemonEvent::Derived(
                    DerivedEvent::ActivityChanged {
                        timestamp: Utc::now(),
                        old_activity,
                        new_activity: ActivityType::Idle,
                        confidence: 1.0,
                    }
                ))?;
            }

            _ => {}
        }

        // 持久化 CoreState
        self.persistence.save(&state).await?;
        Ok(())
    }

    /// 批量处理高频事件
    async fn process_batch(&self, events: &[DaemonEvent]) -> Result<()> {
        let mut context = self.context.write().await;

        // 规则 3: 文件修改模式 → 推理编程语言
        let fs_events: Vec<_> = events.iter()
            .filter_map(|e| match e {
                DaemonEvent::Raw(RawEvent::FsEvent { path, .. }) => Some(path),
                _ => None,
            })
            .collect();

        if let Some(language) = self.infer_language(&fs_events) {
            context.dominant_language = Some(language);

            // 如果当前在编程活动，更新语言信息
            let mut state = self.state.write().await;
            if let ActivityType::Programming { language: lang, .. } = &mut state.activity {
                *lang = Some(context.dominant_language.clone().unwrap());
                self.persistence.save(&state).await?;
            }
        }

        Ok(())
    }

    /// 定期安全网推理
    async fn periodic_inference(&self) -> Result<()> {
        let state = self.state.read().await;
        let context = self.context.read().await;

        // 规则 4: 长时间无键盘/鼠标活动 → 可能进入 Meeting
        if state.activity == ActivityType::Idle {
            // 这里可以结合更多启发式规则
        }

        Ok(())
    }
}
```

### 2.5 持久化策略

```rust
pub struct StatePersistence {
    db_path: PathBuf,
}

impl StatePersistence {
    /// 保存 CoreState 到 JSON 文件
    pub async fn save(&self, state: &CoreState) -> Result<()> {
        let json = serde_json::to_string_pretty(state)?;
        tokio::fs::write(&self.db_path, json).await?;
        Ok(())
    }

    /// 恢复 CoreState
    pub async fn restore(&self) -> Result<CoreState> {
        if !self.db_path.exists() {
            return Ok(CoreState::default());
        }

        let json = tokio::fs::read_to_string(&self.db_path).await?;
        let mut state: CoreState = serde_json::from_str(&json)?;

        // 清理过期的 pending_actions
        state.prune_expired();

        Ok(state)
    }
}

impl CoreState {
    /// 清理过期的待处理动作
    pub fn prune_expired(&mut self) {
        let now = Utc::now();
        self.pending_actions.retain(|action| {
            action.expires_at.map_or(true, |expires| expires > now)
        });
    }
}
```

### 2.6 与 Phase 2 的集成

WorldModel 复用 Phase 2 已实现的基础设施：

```rust
// 启动流程
pub async fn start_worldmodel(
    event_bus: Arc<DaemonEventBus>,
    config: WorldModelConfig,
) -> Result<Arc<WorldModel>> {
    let persistence = Arc::new(StatePersistence::new(
        config.state_path.unwrap_or_else(|| {
            dirs::home_dir().unwrap().join(".aether/worldmodel_state.json")
        })
    ));

    let core_state = Arc::new(RwLock::new(persistence.restore().await?));

    let worldmodel = Arc::new(WorldModel {
        state: core_state,
        context: Arc::new(RwLock::new(EnhancedContext::default())),
        cache: Arc::new(Mutex::new(InferenceCache::new())),
        event_bus: event_bus.clone(),
        persistence,
    });

    let wm = worldmodel.clone();
    tokio::spawn(async move {
        if let Err(e) = wm.run().await {
            error!("WorldModel error: {}", e);
        }
    });

    Ok(worldmodel)
}
```

---

## Part 3: Dispatcher 详细设计

### 3.1 核心职责

Dispatcher 是 Aleph 的"执行中枢"，负责：
- **订阅** DaemonEventBus 的 Derived Events
- **评估** PolicyEngine 的规则匹配
- **调度** 高风险动作（需用户确认）或低风险动作（自动执行）
- **维护** DispatcherMode 状态机（Running/Reconciling）

**关键原则**：Dispatcher **只做决策和调度**，不做推理。推理由 WorldModel 完成。

### 3.2 DispatcherMode 状态机

```rust
/// Dispatcher 的运行模式
#[derive(Debug, Clone, PartialEq)]
pub enum DispatcherMode {
    /// 正常运行模式：监听 Derived Events，触发 Policy 评估
    Running,

    /// 对账模式：逻辑阻塞新的主动触发，专注处理待确认动作
    /// 注意：IPC 仍可正常工作，不会物理阻塞进程
    Reconciling {
        /// 待确认的高风险动作列表
        pending_high_risk: Vec<PendingAction>,

        /// 对账开始时间
        started_at: DateTime<Utc>,
    },
}

impl Dispatcher {
    /// 设置 Dispatcher 模式
    pub async fn set_mode(&self, mode: DispatcherMode) {
        let mut current_mode = self.mode.write().await;

        match (&*current_mode, &mode) {
            (DispatcherMode::Running, DispatcherMode::Reconciling { .. }) => {
                info!("Entering Reconciling mode - pausing proactive triggers");
            }
            (DispatcherMode::Reconciling { .. }, DispatcherMode::Running) => {
                info!("Exiting Reconciling mode - resuming proactive triggers");
            }
            _ => {}
        }

        *current_mode = mode;
    }

    /// 判断是否应该处理新的 Derived Event
    fn should_process_event(&self, mode: &DispatcherMode) -> bool {
        matches!(mode, DispatcherMode::Running)
    }
}
```

### 3.3 Policy Engine 实现

```rust
/// Policy trait - MVP 使用硬编码规则，未来可扩展为动态配置
#[async_trait]
pub trait Policy: Send + Sync {
    /// Policy 名称
    fn name(&self) -> &str;

    /// 评估是否应该触发动作
    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction>;
}

/// 提议的动作
#[derive(Debug, Clone)]
pub struct ProposedAction {
    pub action_type: ActionType,
    pub reason: String,
    pub risk_level: RiskLevel,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionType {
    /// 静音系统音频
    MuteSystemAudio,

    /// 取消静音
    UnmuteSystemAudio,

    /// 开启勿扰模式
    EnableDoNotDisturb,

    /// 关闭勿扰模式
    DisableDoNotDisturb,

    /// 通知用户（低侵入）
    NotifyUser { message: String, priority: NotificationPriority },

    /// 调整屏幕亮度
    AdjustBrightness { level: u8 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum RiskLevel {
    /// 低风险：可自动执行，用户事后可撤销
    Low,

    /// 中风险：延迟通知，等待用户空闲时确认
    Medium,

    /// 高风险：立即询问，Reconciling 模式阻塞其他触发
    High,
}

/// PolicyEngine 管理所有 Policy 实例
pub struct PolicyEngine {
    policies: Vec<Box<dyn Policy>>,
}

impl PolicyEngine {
    /// MVP: 注册 5-10 个硬编码核心规则
    pub fn new_mvp() -> Self {
        let policies: Vec<Box<dyn Policy>> = vec![
            Box::new(MeetingMutePolicy),
            Box::new(LowBatteryPolicy),
            Box::new(FocusModePolicy),
            Box::new(IdleCleanupPolicy),
            Box::new(HighCpuAlertPolicy),
        ];

        Self { policies }
    }

    /// 评估所有 Policy，返回匹配的动作
    pub fn evaluate_all(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Vec<ProposedAction> {
        self.policies
            .iter()
            .filter_map(|policy| policy.evaluate(context, event))
            .collect()
    }
}
```

### 3.4 MVP 核心规则示例

```rust
/// Rule 1: 进入会议时自动静音
struct MeetingMutePolicy;

#[async_trait]
impl Policy for MeetingMutePolicy {
    fn name(&self) -> &str {
        "Auto-Mute in Meeting"
    }

    fn evaluate(
        &self,
        _context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged { new_activity, .. } = event {
            if matches!(new_activity, ActivityType::Meeting { .. }) {
                return Some(ProposedAction {
                    action_type: ActionType::MuteSystemAudio,
                    reason: "User entered a meeting".into(),
                    risk_level: RiskLevel::Low,
                    metadata: HashMap::new(),
                });
            }
        }
        None
    }
}

/// Rule 2: 低电量时通知用户
struct LowBatteryPolicy;

#[async_trait]
impl Policy for LowBatteryPolicy {
    fn name(&self) -> &str {
        "Low Battery Alert"
    }

    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let Some(battery_level) = context.system_constraint.battery_level {
            if battery_level < 20 {
                return Some(ProposedAction {
                    action_type: ActionType::NotifyUser {
                        message: format!("Battery level low: {}%", battery_level),
                        priority: NotificationPriority::High,
                    },
                    reason: "Battery level below 20%".into(),
                    risk_level: RiskLevel::Low,
                    metadata: [("battery_level".into(), battery_level.into())]
                        .into_iter()
                        .collect(),
                });
            }
        }
        None
    }
}

/// Rule 3: 编程会话中 CPU 高负载时启用专注模式
struct FocusModePolicy;

#[async_trait]
impl Policy for FocusModePolicy {
    fn name(&self) -> &str {
        "Focus Mode for High CPU Tasks"
    }

    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged { new_activity, .. } = event {
            if matches!(new_activity, ActivityType::Programming { .. }) {
                if context.system_constraint.cpu_usage > 70.0 {
                    return Some(ProposedAction {
                        action_type: ActionType::EnableDoNotDisturb,
                        reason: "High CPU usage detected during programming".into(),
                        risk_level: RiskLevel::Medium,
                        metadata: [("cpu_usage".into(), context.system_constraint.cpu_usage.into())]
                            .into_iter()
                            .collect(),
                    });
                }
            }
        }
        None
    }
}

/// Rule 4: 空闲超过 30 分钟时清理临时文件
struct IdleCleanupPolicy;

#[async_trait]
impl Policy for IdleCleanupPolicy {
    fn name(&self) -> &str {
        "Idle Cleanup"
    }

    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged { new_activity, .. } = event {
            if matches!(new_activity, ActivityType::Idle) {
                if context.activity_duration > Duration::minutes(30) {
                    return Some(ProposedAction {
                        action_type: ActionType::NotifyUser {
                            message: "Clean up temporary files?".into(),
                            priority: NotificationPriority::Low,
                        },
                        reason: "System idle for 30+ minutes".into(),
                        risk_level: RiskLevel::Medium,
                        metadata: HashMap::new(),
                    });
                }
            }
        }
        None
    }
}

/// Rule 5: 高 CPU 负载时警告
struct HighCpuAlertPolicy;

#[async_trait]
impl Policy for HighCpuAlertPolicy {
    fn name(&self) -> &str {
        "High CPU Alert"
    }

    fn evaluate(
        &self,
        context: &EnhancedContext,
        _event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if context.system_constraint.cpu_usage > 90.0 {
            return Some(ProposedAction {
                action_type: ActionType::NotifyUser {
                    message: format!("CPU usage at {:.1}%", context.system_constraint.cpu_usage),
                    priority: NotificationPriority::High,
                },
                reason: "CPU usage exceeds 90%".into(),
                risk_level: RiskLevel::Low,
                metadata: [("cpu_usage".into(), context.system_constraint.cpu_usage.into())]
                    .into_iter()
                    .collect(),
            });
        }
        None
    }
}
```

### 3.5 Dispatcher 主循环

```rust
pub struct Dispatcher {
    mode: Arc<RwLock<DispatcherMode>>,
    policy_engine: Arc<PolicyEngine>,
    event_bus: Arc<DaemonEventBus>,
    worldmodel: Arc<WorldModel>,
    executor: Arc<ActionExecutor>,
}

impl Dispatcher {
    pub async fn run(&self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();

        loop {
            let mode = self.mode.read().await.clone();

            tokio::select! {
                // 监听 Derived Events
                Ok(event) = rx.recv() => {
                    // 只处理 Derived Events
                    let derived_event = match event {
                        DaemonEvent::Derived(e) => e,
                        _ => continue,
                    };

                    // Reconciling 模式下不处理新事件
                    if !self.should_process_event(&mode) {
                        debug!("Skipping event in Reconciling mode");
                        continue;
                    }

                    // 获取增强上下文
                    let context = self.worldmodel.get_context().await;

                    // 评估所有 Policy
                    let proposed_actions = self.policy_engine.evaluate_all(&context, &derived_event);

                    // 按风险级别分类处理
                    for action in proposed_actions {
                        self.handle_action(action).await?;
                    }
                }
            }
        }
    }

    async fn handle_action(&self, action: ProposedAction) -> Result<()> {
        match action.risk_level {
            RiskLevel::Low => {
                // 低风险：自动执行
                info!("Auto-executing low-risk action: {:?}", action.action_type);
                self.executor.execute(action).await?;
            }

            RiskLevel::Medium => {
                // 中风险：加入延迟通知队列
                info!("Queuing medium-risk action for lazy review: {:?}", action.action_type);
                self.worldmodel.add_pending_action(action, None).await?;

                // 异步等待用户空闲时通知
                let executor = self.executor.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    // TODO: 检查用户是否空闲，然后通知
                });
            }

            RiskLevel::High => {
                // 高风险：立即切换到 Reconciling 模式
                info!("High-risk action detected, entering Reconciling mode");

                // 将动作加入 CoreState
                self.worldmodel.add_pending_action(action.clone(), Some(Duration::hours(24))).await?;

                // 切换模式
                self.set_mode(DispatcherMode::Reconciling {
                    pending_high_risk: vec![action.clone()],
                    started_at: Utc::now(),
                }).await;

                // 通过 IPC 通知客户端（Gateway/macOS App）
                self.notify_urgent_action(action).await?;
            }
        }

        Ok(())
    }

    /// 通过 IPC 通知高风险动作
    async fn notify_urgent_action(&self, action: ProposedAction) -> Result<()> {
        // 这里需要集成 Gateway 的 IPC 机制
        // 暂时使用 placeholder
        info!("IPC: Urgent action notification: {:?}", action);
        Ok(())
    }
}
```

### 3.6 ActionExecutor 实现

```rust
/// 执行动作的具体实现
pub struct ActionExecutor {
    // 可能需要访问系统 API 的句柄
}

impl ActionExecutor {
    pub async fn execute(&self, action: ProposedAction) -> Result<()> {
        match action.action_type {
            ActionType::MuteSystemAudio => {
                #[cfg(target_os = "macos")]
                {
                    use std::process::Command;
                    Command::new("osascript")
                        .arg("-e")
                        .arg("set volume output muted true")
                        .output()?;
                }
                info!("Executed: Mute system audio");
            }

            ActionType::UnmuteSystemAudio => {
                #[cfg(target_os = "macos")]
                {
                    use std::process::Command;
                    Command::new("osascript")
                        .arg("-e")
                        .arg("set volume output muted false")
                        .output()?;
                }
                info!("Executed: Unmute system audio");
            }

            ActionType::EnableDoNotDisturb => {
                #[cfg(target_os = "macos")]
                {
                    // macOS 13+ 使用 Focus API
                    use std::process::Command;
                    Command::new("shortcuts")
                        .arg("run")
                        .arg("Set Focus")
                        .output()?;
                }
                info!("Executed: Enable Do Not Disturb");
            }

            ActionType::NotifyUser { message, priority } => {
                // 通过 Gateway 发送通知
                info!("Notification [{:?}]: {}", priority, message);
            }

            ActionType::AdjustBrightness { level } => {
                #[cfg(target_os = "macos")]
                {
                    use std::process::Command;
                    let brightness = (level as f64) / 100.0;
                    Command::new("brightness")
                        .arg(brightness.to_string())
                        .output()?;
                }
                info!("Executed: Adjust brightness to {}", level);
            }

            _ => {
                warn!("Unimplemented action type: {:?}", action.action_type);
            }
        }

        Ok(())
    }
}
```

---

## Part 4: Reconciliation 机制详细设计

### 4.1 核心理念

Reconciliation 机制确保 Daemon 重启后能够：
- **恢复上下文**：从持久化的 CoreState 中恢复待处理动作
- **询问用户**：对高风险动作进行确认，避免误操作
- **避免阻塞**：通过 DispatcherMode 状态机实现逻辑阻塞，保持 IPC 畅通

**关键约束**：
- Daemon 进程本身 **不能被物理阻塞**（会导致 IPC 死锁）
- 使用 **DispatcherMode::Reconciling** 逻辑阻塞新的主动触发
- IPC 仍可正常工作，客户端可以发送用户响应

### 4.2 启动时对账流程

```rust
/// Daemon 启动时的 Reconciliation 流程
pub async fn daemon_startup(config: DaemonConfig) -> Result<()> {
    // Step 1: 恢复 CoreState
    let persistence = StatePersistence::new(config.state_path);
    let mut core_state = persistence.restore().await?;

    // Step 2: 清理过期的 pending_actions
    core_state.prune_expired();
    info!("Restored CoreState with {} pending actions", core_state.pending_actions.len());

    // Step 3: 启动基础设施
    let event_bus = Arc::new(DaemonEventBus::new(1000));
    let ipc_server = start_ipc_server(&config).await?;

    // Step 4: 启动 Perception Layer
    start_perception_layer(&event_bus, &config.perception).await?;

    // Step 5: 启动 WorldModel
    let worldmodel = start_worldmodel(event_bus.clone(), config.worldmodel).await?;
    worldmodel.restore_state(core_state.clone()).await?;

    // Step 6: 启动 Dispatcher
    let dispatcher = Arc::new(Dispatcher::new(
        event_bus.clone(),
        worldmodel.clone(),
        config.dispatcher,
    ));

    // Step 7: 分级处理 pending_actions
    let (high_risk, medium_risk, low_risk) = classify_actions(&core_state.pending_actions);

    if !high_risk.is_empty() {
        // 有高风险动作 → 进入 Reconciling 模式
        info!("Found {} high-risk pending actions, entering Reconciling mode", high_risk.len());

        dispatcher.set_mode(DispatcherMode::Reconciling {
            pending_high_risk: high_risk.clone(),
            started_at: Utc::now(),
        }).await;

        // 通过 IPC 通知客户端（立即弹窗）
        ipc_server.notify_urgent_actions(&high_risk).await?;
    } else {
        // 无高风险动作 → 直接进入 Running 模式
        info!("No high-risk actions, entering Running mode");
        dispatcher.set_mode(DispatcherMode::Running).await;
    }

    // Step 8: 异步处理中风险动作（延迟通知）
    if !medium_risk.is_empty() {
        let ipc = ipc_server.clone();
        tokio::spawn(async move {
            // 等待用户空闲（例如：30 秒无键盘/鼠标活动）
            wait_for_user_idle(Duration::from_secs(30)).await;

            // 发送低优先级通知
            if let Err(e) = ipc.notify_lazy_review(&medium_risk).await {
                error!("Failed to send lazy review notification: {}", e);
            }
        });
    }

    // Step 9: 低风险动作自动过期（无需通知）
    info!("Low-risk actions ({}) auto-expired", low_risk.len());

    // Step 10: 启动 Dispatcher 主循环
    let dispatcher_task = {
        let d = dispatcher.clone();
        tokio::spawn(async move {
            if let Err(e) = d.run().await {
                error!("Dispatcher error: {}", e);
            }
        })
    };

    // Step 11: 等待信号（graceful shutdown）
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    Ok(())
}

/// 按风险级别分类待处理动作
fn classify_actions(actions: &[PendingAction]) -> (Vec<PendingAction>, Vec<PendingAction>, Vec<PendingAction>) {
    let mut high_risk = Vec::new();
    let mut medium_risk = Vec::new();
    let mut low_risk = Vec::new();

    for action in actions {
        match action.risk_level {
            RiskLevel::High => high_risk.push(action.clone()),
            RiskLevel::Medium => medium_risk.push(action.clone()),
            RiskLevel::Low => low_risk.push(action.clone()),
        }
    }

    (high_risk, medium_risk, low_risk)
}

/// 等待用户空闲
async fn wait_for_user_idle(idle_duration: Duration) {
    // TODO: 集成 macOS IOHIDSystem 检测键盘/鼠标活动
    // 这里简化为固定延迟
    tokio::time::sleep(idle_duration).await;
}
```

### 4.3 用户响应处理

```rust
/// 用户对待处理动作的响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserResponse {
    /// 同意执行
    Approve { action_id: String },

    /// 拒绝执行
    Reject { action_id: String, reason: Option<String> },

    /// 延迟决策（稍后提醒）
    Defer { action_id: String, remind_after: Duration },
}

impl Dispatcher {
    /// 处理用户响应（通过 IPC 调用）
    pub async fn handle_user_response(&self, response: UserResponse) -> Result<()> {
        match response {
            UserResponse::Approve { action_id } => {
                // 从 CoreState 中找到对应动作
                let action = self.worldmodel.remove_pending_action(&action_id).await?;

                // 执行动作
                info!("User approved action: {:?}", action.action_type);
                self.executor.execute(ProposedAction {
                    action_type: action.action_type,
                    reason: action.reason,
                    risk_level: action.risk_level,
                    metadata: HashMap::new(),
                }).await?;

                // 检查是否还有其他高风险动作
                self.check_and_exit_reconciling().await?;
            }

            UserResponse::Reject { action_id, reason } => {
                // 从 CoreState 中移除动作
                self.worldmodel.remove_pending_action(&action_id).await?;
                info!("User rejected action: {} (reason: {:?})", action_id, reason);

                // 检查是否还有其他高风险动作
                self.check_and_exit_reconciling().await?;
            }

            UserResponse::Defer { action_id, remind_after } => {
                // 更新动作的 expires_at 时间
                let new_expires = Utc::now() + remind_after;
                self.worldmodel.update_pending_action_expiry(&action_id, new_expires).await?;

                info!("User deferred action: {} (remind after {:?})", action_id, remind_after);

                // 从 Reconciling 列表中移除，但保留在 CoreState 中
                self.check_and_exit_reconciling().await?;
            }
        }

        Ok(())
    }

    /// 检查是否应该退出 Reconciling 模式
    async fn check_and_exit_reconciling(&self) -> Result<()> {
        let mut mode = self.mode.write().await;

        if let DispatcherMode::Reconciling { pending_high_risk, .. } = &*mode {
            // 获取最新的 CoreState
            let core_state = self.worldmodel.get_core_state().await;

            // 检查是否还有高风险动作
            let remaining_high_risk: Vec<_> = core_state.pending_actions.iter()
                .filter(|a| a.risk_level == RiskLevel::High)
                .collect();

            if remaining_high_risk.is_empty() {
                // 无高风险动作 → 退出 Reconciling 模式
                info!("All high-risk actions resolved, exiting Reconciling mode");
                *mode = DispatcherMode::Running;
            } else {
                // 更新 pending_high_risk 列表
                info!("{} high-risk actions remaining", remaining_high_risk.len());
            }
        }

        Ok(())
    }
}
```

### 4.4 IPC 通知接口

```rust
/// IPC 服务器（Gateway 的一部分）
pub struct IpcServer {
    // WebSocket connections, channels, etc.
}

impl IpcServer {
    /// 通知客户端有紧急的高风险动作需要确认
    pub async fn notify_urgent_actions(&self, actions: &[PendingAction]) -> Result<()> {
        let notification = json!({
            "type": "urgent_actions",
            "actions": actions.iter().map(|a| json!({
                "id": a.id(),
                "action_type": a.action_type,
                "reason": a.reason,
                "created_at": a.created_at,
                "expires_at": a.expires_at,
            })).collect::<Vec<_>>(),
        });

        // 通过 Gateway 推送到所有已连接的客户端
        self.broadcast(notification).await?;

        Ok(())
    }

    /// 通知客户端有中风险动作需要延迟审核
    pub async fn notify_lazy_review(&self, actions: &[PendingAction]) -> Result<()> {
        let notification = json!({
            "type": "lazy_review",
            "actions": actions.iter().map(|a| json!({
                "id": a.id(),
                "action_type": a.action_type,
                "reason": a.reason,
                "created_at": a.created_at,
            })).collect::<Vec<_>>(),
        });

        // 低优先级通知（不打断用户）
        self.send_low_priority(notification).await?;

        Ok(())
    }
}
```

### 4.5 超时和过期策略

```rust
impl PendingAction {
    /// 生成唯一 ID
    pub fn id(&self) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}{}", self.action_type, self.created_at));
        format!("{:x}", hasher.finalize())[..16].to_string()
    }

    /// 判断动作是否已过期
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

impl CoreState {
    /// 清理过期的待处理动作
    pub fn prune_expired(&mut self) {
        let before_count = self.pending_actions.len();
        self.pending_actions.retain(|action| !action.is_expired());
        let removed_count = before_count - self.pending_actions.len();

        if removed_count > 0 {
            info!("Pruned {} expired pending actions", removed_count);
        }
    }
}

/// 后台定期清理过期动作
pub async fn start_expiry_cleanup(worldmodel: Arc<WorldModel>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let mut core_state = worldmodel.get_core_state().await;
            let before_count = core_state.pending_actions.len();
            core_state.prune_expired();
            let after_count = core_state.pending_actions.len();

            if before_count != after_count {
                // 持久化更新后的状态
                worldmodel.update_core_state(core_state).await.ok();
            }
        }
    });
}
```

### 4.6 完整的启动序列图

```
User启动Daemon → Daemon读取CoreState → 发现3个PendingAction（2高+1中）
                                                    ↓
                              设置DispatcherMode::Reconciling(2高风险)
                                                    ↓
                              启动Perception Layer（Level 0 watchers运行）
                                                    ↓
                              启动WorldModel（订阅RawEvents，但不发布DerivedEvents）
                                                    ↓
                              启动Dispatcher（逻辑阻塞新触发，等待用户响应）
                                                    ↓
                              IPC推送紧急通知 → macOS App弹窗
                                                    ↓
                              用户点击"Approve" → Dispatcher执行动作 → 移除PendingAction
                                                    ↓
                              检查剩余高风险动作 → 还有1个 → 保持Reconciling模式
                                                    ↓
                              用户点击"Reject" → 移除PendingAction
                                                    ↓
                              检查剩余高风险动作 → 0个 → 切换到Running模式
                                                    ↓
                              Dispatcher开始监听DerivedEvents，恢复主动触发
                                                    ↓
                              异步任务：等待用户空闲30秒 → 发送中风险动作的低优先级通知
```

### 4.7 关键设计要点

| 要点 | 实现方式 |
|------|----------|
| **物理非阻塞** | Daemon 进程始终可响应 IPC 请求 |
| **逻辑阻塞** | DispatcherMode::Reconciling 时不处理新的 DerivedEvent |
| **分级通知** | 高风险立即弹窗，中风险延迟通知，低风险自动过期 |
| **持久化** | CoreState 持久化到 JSON，每次更新后保存 |
| **自动清理** | 后台任务每 60 秒清理过期动作 |
| **用户控制** | Approve/Reject/Defer 三种响应，完全透明 |

---

## Part 5: 数据结构与 API 定义

### 5.1 Derived Events 完整定义

```rust
/// WorldModel 发布的派生事件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DerivedEvent {
    /// 用户活动发生变化
    ActivityChanged {
        timestamp: DateTime<Utc>,
        old_activity: ActivityType,
        new_activity: ActivityType,
        confidence: f64, // 0.0 ~ 1.0
    },

    /// 编程会话开始
    ProgrammingSessionStarted {
        timestamp: DateTime<Utc>,
        session_id: String,
        language: Option<String>,
        project_root: Option<PathBuf>,
    },

    /// 编程会话结束
    ProgrammingSessionEnded {
        timestamp: DateTime<Utc>,
        session_id: String,
        duration: Duration,
        language: Option<String>,
    },

    /// 系统资源压力变化
    ResourcePressureChanged {
        timestamp: DateTime<Utc>,
        pressure_type: PressureType,
        old_level: PressureLevel,
        new_level: PressureLevel,
    },

    /// 用户进入/退出会议
    MeetingStateChanged {
        timestamp: DateTime<Utc>,
        is_in_meeting: bool,
        participants: Option<usize>,
    },

    /// 用户空闲状态变化
    IdleStateChanged {
        timestamp: DateTime<Utc>,
        is_idle: bool,
        idle_duration: Option<Duration>,
    },

    /// 批量聚合事件（高频事件的统计摘要）
    Aggregated {
        timestamp: DateTime<Utc>,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
        event_type: String,
        event_count: usize,
        summary: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PressureType {
    Cpu,
    Memory,
    Battery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PressureLevel {
    Normal,
    Warning,
    Critical,
}
```

### 5.2 核心数据结构补充定义

```rust
/// 通知优先级
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotificationPriority {
    Low,      // 可延迟，不打断用户
    Normal,   // 正常通知
    High,     // 紧急通知，立即显示
}

/// 循环缓冲区（用于 InferenceCache）
pub struct CircularBuffer<T> {
    buffer: Vec<T>,
    capacity: usize,
    head: usize,
}

impl<T: Clone> CircularBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            capacity,
            head: 0,
        }
    }

    pub fn push(&mut self, item: T) {
        if self.buffer.len() < self.capacity {
            self.buffer.push(item);
        } else {
            self.buffer[self.head] = item;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buffer.iter()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

/// 置信度分数
#[derive(Debug, Clone)]
pub struct ConfidenceScore {
    pub value: f64,
    pub last_updated: DateTime<Utc>,
}

/// 事件计数器（用于批量处理）
#[derive(Debug, Clone)]
pub struct Counter {
    pub count: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl Counter {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            count: 0,
            first_seen: now,
            last_seen: now,
        }
    }

    pub fn increment(&mut self) {
        self.count += 1;
        self.last_seen = Utc::now();
    }
}
```

### 5.3 配置结构

```rust
/// WorldModel 配置
#[derive(Debug, Clone, Deserialize)]
pub struct WorldModelConfig {
    /// 状态文件路径（默认：~/.aleph/worldmodel_state.json）
    pub state_path: Option<PathBuf>,

    /// 批量处理间隔（秒）
    #[serde(default = "default_batch_interval")]
    pub batch_interval: u64,

    /// 定期推理间隔（秒）
    #[serde(default = "default_periodic_interval")]
    pub periodic_interval: u64,

    /// InferenceCache 缓冲区大小
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,

    /// 活动推理的置信度阈值
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

fn default_batch_interval() -> u64 { 5 }
fn default_periodic_interval() -> u64 { 30 }
fn default_cache_size() -> usize { 100 }
fn default_confidence_threshold() -> f64 { 0.7 }

impl Default for WorldModelConfig {
    fn default() -> Self {
        Self {
            state_path: None,
            batch_interval: default_batch_interval(),
            periodic_interval: default_periodic_interval(),
            cache_size: default_cache_size(),
            confidence_threshold: default_confidence_threshold(),
        }
    }
}

/// Dispatcher 配置
#[derive(Debug, Clone, Deserialize)]
pub struct DispatcherConfig {
    /// 是否启用 Dispatcher
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// 用户空闲检测阈值（秒）
    #[serde(default = "default_idle_threshold")]
    pub idle_threshold: u64,

    /// 高风险动作的默认过期时间（小时）
    #[serde(default = "default_high_risk_expiry")]
    pub high_risk_expiry_hours: u64,

    /// 中风险动作的默认过期时间（小时）
    #[serde(default = "default_medium_risk_expiry")]
    pub medium_risk_expiry_hours: u64,

    /// 是否启用自动执行低风险动作
    #[serde(default = "default_auto_execute_low_risk")]
    pub auto_execute_low_risk: bool,
}

fn default_enabled() -> bool { true }
fn default_idle_threshold() -> u64 { 30 }
fn default_high_risk_expiry() -> u64 { 24 }
fn default_medium_risk_expiry() -> u64 { 12 }
fn default_auto_execute_low_risk() -> bool { true }

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            idle_threshold: default_idle_threshold(),
            high_risk_expiry_hours: default_high_risk_expiry(),
            medium_risk_expiry_hours: default_medium_risk_expiry(),
            auto_execute_low_risk: default_auto_execute_low_risk(),
        }
    }
}

/// Daemon 完整配置
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    /// Perception Layer 配置（Phase 2 已实现）
    pub perception: PerceptionConfig,

    /// WorldModel 配置（Phase 3）
    #[serde(default)]
    pub worldmodel: WorldModelConfig,

    /// Dispatcher 配置（Phase 4）
    #[serde(default)]
    pub dispatcher: DispatcherConfig,

    /// 状态持久化路径
    pub state_path: PathBuf,
}
```

### 5.4 WorldModel 核心 API

```rust
impl WorldModel {
    /// 获取当前 CoreState（只读）
    pub async fn get_core_state(&self) -> CoreState {
        self.state.read().await.clone()
    }

    /// 更新 CoreState（写入）
    pub async fn update_core_state(&self, state: CoreState) -> Result<()> {
        let mut current = self.state.write().await;
        *current = state;
        self.persistence.save(&current).await?;
        Ok(())
    }

    /// 获取 EnhancedContext（只读）
    pub async fn get_context(&self) -> EnhancedContext {
        self.context.read().await.clone()
    }

    /// 添加待处理动作到 CoreState
    pub async fn add_pending_action(
        &self,
        action: ProposedAction,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let mut state = self.state.write().await;

        let pending_action = PendingAction {
            action_type: action.action_type,
            reason: action.reason,
            created_at: Utc::now(),
            expires_at: ttl.map(|d| Utc::now() + d),
            risk_level: action.risk_level,
        };

        state.pending_actions.push(pending_action);
        self.persistence.save(&state).await?;

        Ok(())
    }

    /// 移除待处理动作（通过 ID）
    pub async fn remove_pending_action(&self, action_id: &str) -> Result<PendingAction> {
        let mut state = self.state.write().await;

        let index = state.pending_actions.iter()
            .position(|a| a.id() == action_id)
            .ok_or_else(|| anyhow!("Action not found: {}", action_id))?;

        let action = state.pending_actions.remove(index);
        self.persistence.save(&state).await?;

        Ok(action)
    }

    /// 更新待处理动作的过期时间
    pub async fn update_pending_action_expiry(
        &self,
        action_id: &str,
        new_expiry: DateTime<Utc>,
    ) -> Result<()> {
        let mut state = self.state.write().await;

        let action = state.pending_actions.iter_mut()
            .find(|a| a.id() == action_id)
            .ok_or_else(|| anyhow!("Action not found: {}", action_id))?;

        action.expires_at = Some(new_expiry);
        self.persistence.save(&state).await?;

        Ok(())
    }

    /// 恢复状态（启动时调用）
    pub async fn restore_state(&self, state: CoreState) -> Result<()> {
        let mut current = self.state.write().await;
        *current = state;
        Ok(())
    }
}
```

### 5.5 Dispatcher 核心 API

```rust
impl Dispatcher {
    /// 创建 Dispatcher 实例
    pub fn new(
        event_bus: Arc<DaemonEventBus>,
        worldmodel: Arc<WorldModel>,
        config: DispatcherConfig,
    ) -> Self {
        let policy_engine = Arc::new(PolicyEngine::new_mvp());
        let executor = Arc::new(ActionExecutor::new());

        Self {
            mode: Arc::new(RwLock::new(DispatcherMode::Running)),
            policy_engine,
            event_bus,
            worldmodel,
            executor,
        }
    }

    /// 获取当前模式（只读）
    pub async fn get_mode(&self) -> DispatcherMode {
        self.mode.read().await.clone()
    }

    /// 设置模式（写入）
    pub async fn set_mode(&self, mode: DispatcherMode) {
        // 实现在 Part 3 已定义
    }

    /// 处理用户响应（写入）
    pub async fn handle_user_response(&self, response: UserResponse) -> Result<()> {
        // 实现在 Part 4 已定义
    }

    /// 运行主循环
    pub async fn run(&self) -> Result<()> {
        // 实现在 Part 3 已定义
    }
}
```

### 5.6 错误类型定义

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorldModelError {
    #[error("Failed to load state: {0}")]
    StateLoadError(#[from] std::io::Error),

    #[error("Failed to parse state: {0}")]
    StateParseError(#[from] serde_json::Error),

    #[error("Inference error: {0}")]
    InferenceError(String),

    #[error("Event bus error: {0}")]
    EventBusError(String),
}

#[derive(Error, Debug)]
pub enum DispatcherError {
    #[error("Policy evaluation failed: {0}")]
    PolicyError(String),

    #[error("Action execution failed: {0}")]
    ExecutionError(String),

    #[error("Action not found: {0}")]
    ActionNotFound(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("WorldModel error: {0}")]
    WorldModelError(#[from] WorldModelError),
}
```

### 5.7 模块组织结构

```
core/src/
├── daemon/
│   ├── events.rs                    # [Phase 2] DaemonEvent 定义
│   ├── event_bus.rs                 # [Phase 2] EventBus 实现
│   ├── perception/                  # [Phase 2] Perception Layer
│   │   ├── config.rs
│   │   ├── watcher.rs
│   │   ├── registry.rs
│   │   └── watchers/
│   │       ├── time.rs
│   │       ├── process.rs
│   │       ├── system.rs
│   │       └── filesystem.rs
│   ├── worldmodel/                  # [Phase 3] WorldModel
│   │   ├── mod.rs                   # WorldModel 主结构
│   │   ├── state.rs                 # CoreState/EnhancedContext/InferenceCache
│   │   ├── inference.rs             # 推理规则
│   │   ├── persistence.rs           # StatePersistence
│   │   └── config.rs                # WorldModelConfig
│   └── dispatcher/                  # [Phase 4] Dispatcher
│       ├── mod.rs                   # Dispatcher 主结构
│       ├── mode.rs                  # DispatcherMode 状态机
│       ├── policy.rs                # Policy trait + PolicyEngine
│       ├── policies/                # 具体 Policy 实现
│       │   ├── meeting.rs
│       │   ├── battery.rs
│       │   ├── focus.rs
│       │   ├── idle.rs
│       │   └── cpu.rs
│       ├── executor.rs              # ActionExecutor
│       ├── reconciliation.rs        # Reconciliation 逻辑
│       └── config.rs                # DispatcherConfig
└── lib.rs                           # 导出 pub mod daemon
```

---

## Part 6: Implementation Roadmap

### 6.1 实施顺序

我们将 Phase 3 + 4 的实施分为 **8 个任务**，遵循"先骨架、后血肉"的原则：

#### Task 1: 数据结构定义（骨架）
**目标**：定义所有核心数据结构，确保类型系统完整

**文件**：
- `core/src/daemon/worldmodel/state.rs` - CoreState/EnhancedContext/InferenceCache
- `core/src/daemon/dispatcher/mode.rs` - DispatcherMode
- `core/src/daemon/dispatcher/policy.rs` - Policy trait/ProposedAction/ActionType

**验收标准**：
- 所有结构体实现 `Debug`, `Clone`, `Serialize`, `Deserialize`（如需要）
- 编译通过，无 clippy 警告
- 类型系统与 Phase 2 的 `DaemonEvent` 兼容

---

#### Task 2: StatePersistence（持久化层）
**目标**：实现 CoreState 的 JSON 持久化

**文件**：
- `core/src/daemon/worldmodel/persistence.rs`

**实现**：
- `save()` - 序列化到 JSON 文件
- `restore()` - 反序列化，自动清理过期动作
- 错误处理：文件不存在返回 `default()`

**测试**：
- 单测：保存 → 恢复 → 验证一致性
- 单测：恢复不存在的文件 → 返回默认值
- 单测：`prune_expired()` 正确清理

---

#### Task 3: WorldModel 框架（事件处理循环）
**目标**：实现 WorldModel 的事件订阅和处理框架

**文件**：
- `core/src/daemon/worldmodel/mod.rs`
- `core/src/daemon/worldmodel/config.rs`

**实现**：
- `run()` - 三策略事件循环（立即/批量/定期）
- `is_key_event()` - 关键事件判断
- `process_immediate()` - 立即处理（空实现，Task 4 补充）
- `process_batch()` - 批量处理（空实现，Task 4 补充）
- `periodic_inference()` - 定期推理（空实现，Task 4 补充）

**测试**：
- 集成测试：订阅 EventBus，发送测试事件，验证调用路径
- 单测：`is_key_event()` 正确分类

---

#### Task 4: WorldModel 推理规则（核心逻辑）
**目标**：实现 5 个 MVP 推理规则

**文件**：
- `core/src/daemon/worldmodel/inference.rs`

**规则**：
1. IDE 启动 → `ActivityChanged(Programming)`
2. 显示器休眠 → `ActivityChanged(Idle)`
3. 文件修改模式 → 推理编程语言
4. 长时间无活动 → `IdleStateChanged(true)`
5. CPU/内存/电量变化 → `ResourcePressureChanged`

**测试**：
- 单测：每个规则独立测试（模拟输入 RawEvent → 验证输出 DerivedEvent）
- 集成测试：完整流程（EventBus → WorldModel → 验证 DerivedEvent 发布）

---

#### Task 5: PolicyEngine 和 MVP 规则
**目标**：实现 5 个 MVP Policy 规则

**文件**：
- `core/src/daemon/dispatcher/policies/meeting.rs`
- `core/src/daemon/dispatcher/policies/battery.rs`
- `core/src/daemon/dispatcher/policies/focus.rs`
- `core/src/daemon/dispatcher/policies/idle.rs`
- `core/src/daemon/dispatcher/policies/cpu.rs`

**规则**：
1. 进入会议 → 静音（Low Risk）
2. 低电量 → 通知（Low Risk）
3. 编程 + 高 CPU → 专注模式（Medium Risk）
4. 空闲 30 分钟 → 清理建议（Medium Risk）
5. CPU > 90% → 警告（Low Risk）

**测试**：
- 单测：每个 Policy 独立测试（模拟 DerivedEvent + EnhancedContext → 验证 ProposedAction）
- 单测：`PolicyEngine::evaluate_all()` 正确聚合结果

---

#### Task 6: Dispatcher 框架和 ActionExecutor
**目标**：实现 Dispatcher 主循环和动作执行器

**文件**：
- `core/src/daemon/dispatcher/mod.rs`
- `core/src/daemon/dispatcher/executor.rs`

**实现**：
- `run()` - 订阅 DerivedEvents，评估 Policy，按风险级别处理
- `handle_action()` - 分级处理逻辑
- `ActionExecutor::execute()` - 5 个动作类型的实现（macOS AppleScript）

**测试**：
- 单测：`ActionExecutor` 每个动作类型（Mock 系统调用）
- 集成测试：完整流程（DerivedEvent → Policy 匹配 → 动作执行）

---

#### Task 7: Reconciliation 机制
**目标**：实现启动时对账和用户响应处理

**文件**：
- `core/src/daemon/dispatcher/reconciliation.rs`

**实现**：
- `daemon_startup()` - 启动流程（恢复状态 → 分级处理 → 设置模式）
- `handle_user_response()` - 处理 Approve/Reject/Defer
- `check_and_exit_reconciling()` - 自动退出 Reconciling 模式
- `start_expiry_cleanup()` - 后台定期清理

**测试**：
- 单测：`classify_actions()` 正确分级
- 单测：`prune_expired()` 正确清理
- 集成测试：模拟启动流程（有/无高风险动作）
- 集成测试：模拟用户响应（Approve/Reject/Defer）

---

#### Task 8: CLI 集成和端到端测试
**目标**：集成到 `daemon run` 命令，编写端到端测试

**文件**：
- `core/src/daemon/cli.rs` - 修改 `run()` 方法

**修改**：
```rust
pub async fn run() -> Result<()> {
    // [Phase 2] 启动 Perception Layer
    let event_bus = Arc::new(DaemonEventBus::new(1000));
    let perception = start_perception_layer(&event_bus, &config.perception).await?;

    // [Phase 3] 启动 WorldModel
    let worldmodel = start_worldmodel(event_bus.clone(), config.worldmodel).await?;

    // [Phase 4] 启动 Dispatcher（含 Reconciliation）
    daemon_startup(config).await?;

    // Graceful shutdown
    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

**测试**：
- E2E 测试 1：冷启动（无 CoreState）→ 正常运行
- E2E 测试 2：热启动（有高风险动作）→ Reconciling 模式
- E2E 测试 3：完整流程（RawEvent → DerivedEvent → Policy → Action）

---

### 6.2 测试策略

| 测试类型 | 范围 | 工具 | 目标覆盖率 |
|---------|------|------|-----------|
| **单元测试** | 每个模块独立 | `cargo test` | 80%+ |
| **集成测试** | 模块间交互 | `tests/` 目录 | 关键路径 100% |
| **E2E 测试** | 完整系统 | 模拟 Daemon 启动 | 3 个核心场景 |

**测试数据**：
- Mock RawEvents：使用 `tests/fixtures/raw_events.json`
- Mock CoreState：使用 `tests/fixtures/core_state.json`

**CI 集成**：
- 所有 PR 必须通过测试
- Clippy 无警告
- `cargo fmt` 检查

---

### 6.3 里程碑定义

| 里程碑 | 完成标志 | 预期输出 |
|--------|---------|---------|
| **M1: 数据层** | Task 1-2 完成 | 所有数据结构定义，持久化可用 |
| **M2: WorldModel** | Task 3-4 完成 | WorldModel 可监听事件并发布 DerivedEvent |
| **M3: Dispatcher** | Task 5-6 完成 | Dispatcher 可评估 Policy 并执行动作 |
| **M4: 完整闭环** | Task 7-8 完成 | 端到端可用，Reconciliation 正常工作 |

---

### 6.4 验收标准

#### Phase 3 (WorldModel) 验收
- [ ] 可订阅 DaemonEventBus 的 RawEvents
- [ ] 可发布 5 种 DerivedEvent 类型
- [ ] CoreState 正确持久化和恢复
- [ ] 过期动作自动清理
- [ ] 单测覆盖率 > 80%
- [ ] 集成测试覆盖关键路径

#### Phase 4 (Dispatcher) 验收
- [ ] 可订阅 DerivedEvents
- [ ] PolicyEngine 正确评估 5 个 MVP 规则
- [ ] ActionExecutor 可执行 5 种动作类型
- [ ] DispatcherMode 状态机正常切换
- [ ] Reconciliation 正常工作（启动对账 + 用户响应）
- [ ] 单测覆盖率 > 80%
- [ ] E2E 测试覆盖 3 个核心场景

#### 整体验收
- [ ] `cargo build` 无警告
- [ ] `cargo test` 全部通过
- [ ] `cargo clippy` 无警告
- [ ] 文档完整（API docs + 设计文档）
- [ ] CLI 可正常启动和关闭

---

### 6.5 与现有系统的集成点

| 现有系统 | 集成点 | 注意事项 |
|---------|--------|---------|
| **Gateway** | IPC 通知 | 需要添加 `notify_urgent_actions` 和 `notify_lazy_review` RPC 方法 |
| **Perception Layer** | DaemonEventBus | 直接复用，无需修改 |
| **Config System** | `~/.aleph/daemon.toml` | 添加 `[worldmodel]` 和 `[dispatcher]` 配置段 |
| **Memory System** | 未来集成 | Phase 3/4 MVP 不依赖 Memory，Phase 5 可接入 |

---

### 6.6 风险和缓解措施

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| **推理规则过于简单** | 高误报率 | MVP 只实现 5 个高置信度规则，后续迭代优化 |
| **Policy 规则冲突** | 动作混乱 | MVP 使用 Vec 顺序评估，Phase 5 引入优先级 |
| **ActionExecutor 权限不足** | 动作执行失败 | 添加权限检查，失败时降级为通知 |
| **CoreState 过大** | 持久化性能差 | 设计时已限制只存储必需状态（KB 级） |
| **Reconciling 模式卡死** | Daemon 无响应 | 使用逻辑阻塞 + 超时机制（24 小时自动退出） |
| **IPC 死锁** | 系统不可用 | Dispatcher 不物理阻塞进程，IPC 始终可用 |

---

### 6.7 后续迭代方向（Phase 5+）

| 方向 | 描述 |
|------|------|
| **LLM 推理** | WorldModel 调用 LLM 进行复杂上下文推理 |
| **Memory 集成** | 从 Facts DB 检索历史模式 |
| **动态 Policy** | 支持用户自定义规则（DSL 或 YAML） |
| **学习反馈** | 根据用户 Approve/Reject 调整 Policy 权重 |
| **多设备同步** | CoreState 跨设备同步 |

---

## 设计文档总结

这份设计文档涵盖了：

1. **Part 1**: 架构概览 - 数据流、职责分离
2. **Part 2**: WorldModel 详细设计 - 三层状态、推理规则
3. **Part 3**: Dispatcher 详细设计 - Policy Engine、状态机
4. **Part 4**: Reconciliation 机制 - 启动对账、用户响应
5. **Part 5**: 数据结构与 API - 完整定义、错误类型
6. **Part 6**: 实施路线图 - 8 个任务、测试策略、验收标准

**核心设计原则**：
- ✅ 以终为始：从 Dispatcher 需求反推 WorldModel 状态
- ✅ 职责分离：WorldModel 只推理，Dispatcher 只决策
- ✅ 轻量持久化：CoreState 只存储必需数据（KB 级）
- ✅ 逻辑阻塞：通过状态机实现，不物理阻塞进程
- ✅ 分级响应：High/Medium/Low 三级风险处理

**实施准备**：
- 设计已完成并获得批准
- 8 个任务已细化
- 测试策略已明确
- 验收标准已定义

**下一步**：
1. 提交设计文档到 git
2. 创建 worktree 隔离环境
3. 编写详细实施计划
4. 使用 subagent-driven development 执行实施
