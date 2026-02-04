# Proactive AI Architecture Design for Aleph

**Date:** 2026-02-04
**Status:** Approved
**Related:** [PROACTIVE_ARCHITECTURE.md](../PROACTIVE_ARCHITECTURE.md)

## Executive Summary

This document presents the detailed design for evolving Aleph from a reactive CLI tool into a proactive, autonomous AI assistant — a "JARVIS-like" system that runs in the background, perceives the user's environment, maintains context continuity, and autonomously initiates helpful actions without explicit commands.

**Core Philosophy:** Transform AI from a "tool" into a "colleague" — one that not only responds to instructions but also senses, understands, and proactively serves.

---

## 1. Architecture Overview

### 1.1 Design Principles

1. **Invisibility First** — Default zero interruption, interact only when necessary
2. **Semantic Layering** — Clear boundaries: Data → Information → Knowledge → Action
3. **Safety by Design** — Three-tier risk classification + reversible operations
4. **Cognitive Continuity** — Persist cognitive context, no memory loss on restart
5. **Frugal Resources** — Low CPU/memory footprint, respect foreground applications

### 1.2 System Layers

```
┌─────────────────────────────────────────────────────────┐
│ Layer 4: Interaction (交互层)                            │
│  CLI / macOS App / Notifications                         │
└────────────────┬────────────────────────────────────────┘
                 │
┌────────────────┴────────────────────────────────────────┐
│ Layer 3: Cognition (认知层)                              │
│  Dispatcher + PolicyEngine                               │
│  (Decision: "What to do? How to do it?")                 │
└────────────────┬────────────────────────────────────────┘
                 │ subscribes to Derived Events
┌────────────────┴────────────────────────────────────────┐
│ Layer 2: Understanding (理解层)                          │
│  WorldModel (State Inference + Semantic Distillation)    │
└────────────────┬────────────────────────────────────────┘
                 │ subscribes to Raw Events
┌────────────────┴────────────────────────────────────────┐
│ Layer 1: Sensation (感知层)                              │
│  Watchers (FSEvent, Process, Time, SystemState)         │
└─────────────────────────────────────────────────────────┘
```

### 1.3 Core Architectural Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Operational Mode** | Hybrid (Sensing + Time + Learning), prioritize Sensing | Validates architecture completeness while focusing on immediate value |
| **Proactivity Boundary** | Smart Tiering (3-level risk) | Balance between "prediction without nagging" and system safety |
| **State Persistence** | Hybrid Strategy (Physical vs Cognitive) | Distinguish "ground truth" from "cognitive context" |
| **Event Architecture** | Layered Bus (Raw → Derived) | Solve "semantic gap" — data vs information |

---

## 2. Module 1: Daemon Manager

### 2.1 Objective

Ensure Aleph runs continuously as a system service and survives reboots.

### 2.2 Core Components

```rust
// core/src/daemon/mod.rs
pub trait ServiceManager {
    async fn install(&self, config: DaemonConfig) -> Result<()>;
    async fn uninstall(&self) -> Result<()>;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn status(&self) -> ServiceStatus;
}

// Platform implementations
pub struct LaunchdService;  // macOS
pub struct SystemdService;  // Linux
pub struct WindowsService;  // Windows (future)
```

### 2.3 macOS Implementation (LaunchdService)

- **Plist Location:** `~/Library/LaunchAgents/com.aether.daemon.plist`
- **Binary Path:** `~/.aleph/bin/aether-daemon`
- **Launch Trigger:** User login (`RunAtLoad: true`)
- **Crash Recovery:** `KeepAlive: true` ensures auto-restart
- **Resource Limits:**
  - Nice value: 10 (low priority)
  - Soft memory limit: 512MB, hard limit: 1GB

### 2.4 IPC Channel

Daemon listens on Unix Domain Socket: `~/.aleph/daemon.sock`

```rust
// JSON-RPC 2.0 Protocol
enum DaemonCommand {
    GetStatus,
    TriggerAction(ActionRequest),
    Shutdown,
    ReloadConfig,
}
```

### 2.5 Resource Governor

```rust
struct ResourceGovernor {
    cpu_threshold: f32,    // Default 20%
    mem_threshold: u64,    // Default 512MB
    battery_threshold: f32, // Default 20%
}

impl ResourceGovernor {
    // Check every 30 seconds
    async fn check(&self) -> GovernorDecision {
        if battery_low() || system_overloaded() {
            Throttle  // Pause all proactive tasks
        } else {
            Proceed
        }
    }
}
```

---

## 3. Module 2: Perception Layer

### 3.1 Objective

Act as the system's "sensory system", continuously monitoring environment changes and converting them into standardized events.

### 3.2 Core Abstraction

```rust
// core/src/perception/mod.rs
#[async_trait]
pub trait Watcher: Send + Sync {
    fn id(&self) -> &'static str;
    async fn start(&self, event_bus: EventBus) -> Result<()>;
    async fn stop(&self) -> Result<()> { Ok(()) }
    fn health(&self) -> WatcherHealth { WatcherHealth::Healthy }
}
```

### 3.3 MVP-Phase Watchers

#### 3.3.1 ProcessWatcher

```rust
use sysinfo::{System, ProcessExt};

impl ProcessWatcher {
    async fn poll(&self) {
        let mut sys = System::new_all();
        loop {
            sys.refresh_processes();

            for app in &["Code", "Google Chrome", "Zoom"] {
                if let Some(proc) = sys.process_by_name(app) {
                    self.bus.send(RawEvent::ProcessDetected {
                        name: app.to_string(),
                        pid: proc.pid(),
                        cpu_usage: proc.cpu_usage(),
                    });
                }
            }

            sleep(Duration::from_secs(5)).await; // 5s polling
        }
    }
}
```

#### 3.3.2 FSEventWatcher

```rust
use notify::{Watcher as NotifyWatcher, RecursiveMode};

impl FSEventWatcher {
    fn new(paths: Vec<PathBuf>) -> Self {
        // Monitor: ~/Downloads, ~/Desktop, active Git repos
        let mut watcher = notify::recommended_watcher(|res| {
            match res {
                Ok(Event { paths, kind, .. }) => {
                    // Filter noise: .git, .DS_Store, node_modules
                    if !is_noise(&paths[0]) {
                        self.bus.send(RawEvent::FileChanged(paths[0]));
                    }
                }
            }
        })?;
    }
}
```

#### 3.3.3 TimeWatcher

```rust
impl TimeWatcher {
    async fn run(&self) {
        // Heartbeat: every 30 seconds
        let mut heartbeat = interval(Duration::from_secs(30));

        loop {
            heartbeat.tick().await;
            self.bus.send(RawEvent::Heartbeat {
                timestamp: Utc::now(),
            });
        }
    }
}
```

#### 3.3.4 SystemStateWatcher

```rust
impl SystemStateWatcher {
    async fn monitor(&self) {
        loop {
            let battery = battery_level();
            let idle_time = idle_duration();

            self.bus.send(RawEvent::SystemState {
                battery_percent: battery,
                is_idle: idle_time > Duration::from_mins(5),
                network_online: check_network(),
            });

            sleep(Duration::from_secs(60)).await;
        }
    }
}
```

### 3.4 Lifecycle Management

```rust
struct WatcherRegistry {
    watchers: HashMap<String, Box<dyn Watcher>>,
}

impl WatcherRegistry {
    async fn start_all(&mut self) {
        for watcher in self.watchers.values() {
            tokio::spawn(async move {
                if let Err(e) = watcher.start(bus.clone()).await {
                    error!("Watcher {} failed: {}", watcher.id(), e);
                }
            });
        }
    }
}
```

---

## 4. Module 3: WorldModel

### 4.1 Objective

Act as the system's "cerebral cortex", aggregating raw events, inferring user context, and emitting high-level semantic events.

### 4.2 Core Data Structures

```rust
// core/src/context/world_model.rs
pub struct WorldModel {
    // Volatile Layer: Mirror of physical reality
    env: Arc<RwLock<EnvironmentState>>,

    // Persistent Layer: Cognitive context
    memory: Arc<RwLock<CognitiveState>>,

    // Event Bus (for sending Derived Events)
    derived_bus: EventBus,
}

// Volatile State (rebuilt on restart)
#[derive(Default)]
struct EnvironmentState {
    active_processes: HashMap<String, ProcessInfo>,
    recent_file_changes: VecDeque<FileChangeEvent>, // Last 100
    battery_level: Option<f32>,
    is_idle: bool,
    last_activity: Instant,
}

// Persistent State (serialized to disk)
#[derive(Serialize, Deserialize)]
struct CognitiveState {
    current_context: UserContext,
    pending_tasks: Vec<PendingTask>,
    learned_patterns: HashMap<String, UserPreference>,
    goals: Vec<Goal>,
}
```

### 4.3 Context Inference

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserContext {
    Idle,
    Coding { language: String, project: Option<String> },
    Browsing { focus: BrowsingFocus },
    InMeeting,
    Unknown,
}

impl WorldModel {
    async fn infer_context(&self, env: &EnvironmentState) -> UserContext {
        // Rule 1: VS Code running + recent .rs file changes
        if env.active_processes.contains_key("Code") {
            if let Some(lang) = self.detect_language(&env.recent_file_changes) {
                return UserContext::Coding {
                    language: lang,
                    project: self.detect_project()
                };
            }
        }

        // Rule 2: Zoom running + CPU > 30%
        if env.active_processes.contains_key("Zoom") {
            return UserContext::InMeeting;
        }

        // Rule 3: No activity for 5+ minutes
        if env.is_idle && env.last_activity.elapsed() > Duration::from_secs(300) {
            return UserContext::Idle;
        }

        UserContext::Unknown
    }
}
```

### 4.4 Event Processing Loop

```rust
impl WorldModel {
    pub async fn start(self: Arc<Self>, raw_bus: EventBus) {
        let mut rx = raw_bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(AetherEvent::Raw(raw)) => {
                    // 1. Update environment state
                    self.update_environment(raw).await;

                    // 2. Infer new context
                    let env = self.env.read().await;
                    let new_context = self.infer_context(&env).await;

                    // 3. Detect context change
                    let mut memory = self.memory.write().await;
                    if new_context != memory.current_context {
                        info!("Context changed: {:?} -> {:?}",
                              memory.current_context, new_context);

                        memory.current_context = new_context.clone();

                        // 4. Emit high-level event
                        self.derived_bus.send(AetherEvent::Derived(
                            ContextEvent::ActivityChanged(new_context)
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}
```

### 4.5 Persistence Strategy

```rust
impl WorldModel {
    // Save every 5 minutes or on context change
    async fn persist(&self) -> Result<()> {
        let memory = self.memory.read().await;
        let json = serde_json::to_string_pretty(&*memory)?;
        tokio::fs::write("~/.aleph/world_state.json", json).await?;
        Ok(())
    }

    // Load on startup
    async fn restore() -> Result<CognitiveState> {
        let json = tokio::fs::read_to_string("~/.aleph/world_state.json").await?;
        Ok(serde_json::from_str(&json)?)
    }
}
```

---

## 5. Module 4: Dispatcher + PolicyEngine

### 5.1 Objective

Act as the system's "frontal lobe", subscribing to high-level events, evaluating risks, and deciding execution strategies.

### 5.2 Core Architecture

```rust
// core/src/dispatcher/mod.rs
pub struct Dispatcher {
    policy_engine: PolicyEngine,
    agent_executor: AgentExecutor,
    notification_service: NotificationService,
}

impl Dispatcher {
    pub async fn start(self: Arc<Self>, derived_bus: EventBus) {
        let mut rx = derived_bus.subscribe();

        loop {
            match rx.recv().await {
                Ok(AetherEvent::Derived(event)) => {
                    if let Some(action) = self.match_action(&event).await {
                        self.handle_action(action).await;
                    }
                }
                _ => {}
            }
        }
    }
}
```

### 5.3 PolicyEngine — Three-Tier Risk Classification

```rust
// core/src/dispatcher/policy_engine.rs
pub enum RiskLevel {
    Low,    // Read-only, silent execution
    Medium, // Reversible writes, notify after
    High,   // Irreversible, ask before
}

pub struct PolicyEngine {
    user_preferences: HashMap<String, ActionPolicy>,
    resource_governor: ResourceGovernor,
}

pub enum ActionPolicy {
    AlwaysAllow,
    AlwaysDeny,
    AskEveryTime,
    LearnFromHistory(u32), // Auto-downgrade after N approvals
}

impl PolicyEngine {
    pub async fn evaluate(&self, action: &ProposedAction) -> ExecutionStrategy {
        // 1. Check resource governance
        if !self.resource_governor.is_safe_to_run().await {
            return ExecutionStrategy::Defer;
        }

        // 2. Check user preferences
        if let Some(policy) = self.user_preferences.get(action.id) {
            match policy {
                ActionPolicy::AlwaysAllow => return ExecutionStrategy::Silent,
                ActionPolicy::AlwaysDeny => return ExecutionStrategy::Block,
                _ => {}
            }
        }

        // 3. Decide by risk level
        match action.risk_level {
            RiskLevel::Low => ExecutionStrategy::Silent,
            RiskLevel::Medium => ExecutionStrategy::NotifyAfter,
            RiskLevel::High => ExecutionStrategy::AskBefore,
        }
    }
}

pub enum ExecutionStrategy {
    Silent,       // Execute silently
    NotifyAfter,  // Execute first, then notify (with undo)
    AskBefore,    // Ask first, wait for confirmation
    Defer,        // Defer to system idle time
    Block,        // Refuse execution
}
```

### 5.4 Execution Flow

```rust
impl Dispatcher {
    async fn handle_action(&self, action: ProposedAction) {
        let strategy = self.policy_engine.evaluate(&action).await;

        match strategy {
            ExecutionStrategy::Silent => {
                self.execute_silently(action).await;
            }

            ExecutionStrategy::NotifyAfter => {
                let result = self.execute(action.clone()).await;

                self.notification_service.send(Notification {
                    title: "Aleph completed a task",
                    body: action.description,
                    actions: vec![
                        NotificationAction::Undo(action.id),
                        NotificationAction::Dismiss,
                    ],
                });
            }

            ExecutionStrategy::AskBefore => {
                self.notification_service.send_interactive(Notification {
                    title: "Aleph needs your confirmation",
                    body: format!("{}?", action.description),
                    actions: vec![
                        NotificationAction::Approve(action.id),
                        NotificationAction::Deny,
                        NotificationAction::AlwaysAllow,
                    ],
                });
            }

            ExecutionStrategy::Defer => {
                self.pending_queue.push(action).await;
            }

            ExecutionStrategy::Block => {
                debug!("Action {} blocked by policy", action.id);
            }
        }
    }
}
```

---

## 6. Module 5: Startup & Reconciliation

### 6.1 Objective

Implement "restart with memory" — not only restore state but also leverage state differences to proactively provide services.

### 6.2 Startup Flow

```rust
// core/src/daemon/startup.rs
impl AlephDaemon {
    pub async fn start() -> Result<Self> {
        info!("🚀 Aleph Daemon starting...");

        // Phase 1: Load Cognitive State
        let cognitive_state = match WorldModel::restore().await {
            Ok(state) => {
                info!("✅ Restored cognitive state from disk");
                state
            }
            Err(_) => {
                info!("📝 No previous state found, starting fresh");
                CognitiveState::default()
            }
        };

        // Phase 2: Initialize Components
        let event_bus = EventBus::new();
        let world_model = Arc::new(WorldModel::new(cognitive_state));
        let dispatcher = Arc::new(Dispatcher::new());

        // Phase 3: Start Watchers
        let mut watchers = WatcherRegistry::new();
        watchers.register(Box::new(ProcessWatcher::new()));
        watchers.register(Box::new(FSEventWatcher::new()));
        watchers.register(Box::new(TimeWatcher::new()));
        watchers.register(Box::new(SystemStateWatcher::new()));

        info!("👁️  Starting {} watchers...", watchers.len());
        watchers.start_all(event_bus.clone()).await?;

        // Phase 4: Start WorldModel Event Loop
        let wm = world_model.clone();
        tokio::spawn(async move {
            wm.start(event_bus.clone()).await;
        });

        // Phase 5: Start Dispatcher Event Loop
        let dp = dispatcher.clone();
        tokio::spawn(async move {
            dp.start(event_bus.clone()).await;
        });

        // Phase 6: Reconciliation Window (30s)
        info!("⏳ Entering reconciliation phase (30s)...");
        sleep(Duration::from_secs(30)).await;

        // Phase 7: Trigger Reconciliation
        let reconciler = Reconciler::new(world_model.clone());
        reconciler.reconcile().await?;

        info!("✨ Aleph is now active");

        Ok(Self { world_model, dispatcher, watchers, event_bus })
    }
}
```

### 6.3 Reconciliation Logic

```rust
// core/src/daemon/reconciler.rs
pub struct Reconciler {
    world_model: Arc<WorldModel>,
}

impl Reconciler {
    pub async fn reconcile(&self) -> Result<()> {
        let memory = self.world_model.memory.read().await;
        let env = self.world_model.env.read().await;

        info!("🔍 Reconciling cognitive state with physical reality...");

        // Scenario 1: Active task in memory, but environment changed
        if let UserContext::Coding { project, .. } = &memory.current_context {
            if !env.active_processes.contains_key("Code") {
                self.suggest_resume_coding(project.clone()).await?;
            }
        }

        // Scenario 2: Pending confirmation requests
        if !memory.pending_tasks.is_empty() {
            info!("📋 Found {} pending tasks from last session",
                  memory.pending_tasks.len());

            for task in &memory.pending_tasks {
                self.notify_pending_task(task).await?;
            }
        }

        // Scenario 3: Significant environment shift detected
        if self.detect_environment_shift(&memory, &env) {
            info!("🔄 Detected significant environment change since last session");
        }

        Ok(())
    }

    async fn suggest_resume_coding(&self, project: Option<String>) -> Result<()> {
        let message = if let Some(proj) = project {
            format!("检测到你上次在处理项目 {}，需要我帮你打开吗？", proj)
        } else {
            "检测到你上次在写代码，需要恢复开发环境吗？".to_string()
        };

        NotificationService::send_interactive(Notification {
            title: "Aleph - 准备好继续工作了吗？",
            body: message,
            actions: vec![
                NotificationAction::Custom {
                    id: "resume_coding",
                    label: "恢复环境",
                },
                NotificationAction::Dismiss,
            ],
        });

        Ok(())
    }
}
```

### 6.4 Design Highlights

1. **30-Second Warm-up** — Give Watchers time to build accurate current state
2. **Difference as Opportunity** — Reconciler transforms state gaps into proactive service triggers
3. **Non-Intrusive Interaction** — Present differences via dismissible notifications, not forced dialogs

---

## 7. Implementation Roadmap

### 7.1 Phased Implementation

```
Phase 1: Backbone (基础设施)        [2-3 weeks]
  ├─ Daemon Manager (macOS launchd)
  ├─ IPC Channel (Unix Domain Socket)
  └─ Resource Governor

Phase 2: Nervous System (神经系统)  [2-3 weeks]
  ├─ EventBus (layered architecture)
  ├─ ProcessWatcher + TimeWatcher
  └─ Event flow validation (log printing)

Phase 3: Brain (大脑)               [3-4 weeks]
  ├─ WorldModel (state inference)
  ├─ Persistence mechanism
  └─ Reconciliation logic

Phase 4: Action (行动)              [2-3 weeks]
  ├─ Dispatcher + PolicyEngine
  ├─ Notification service integration
  └─ Simple Agent triggering

Phase 5: JARVIS Mode (整合)        [1-2 weeks]
  ├─ Complete startup flow
  ├─ macOS App integration
  └─ User feedback loop
```

### 7.2 Phase 1 Detailed Tasks

```rust
// Milestone 1.1: Service Manager
[ ] Implement LaunchdService trait
[ ] Generate plist file
[ ] CLI commands: aether daemon install/start/stop/status
[ ] Test: Install and restart macOS, verify auto-start

// Milestone 1.2: IPC Channel
[ ] Create ~/.aleph/daemon.sock
[ ] Implement JSON-RPC 2.0 protocol
[ ] CLI-Daemon communication test
[ ] Error handling (friendly message when daemon not running)

// Milestone 1.3: Resource Governor
[ ] Use sysinfo crate to monitor CPU/memory
[ ] Use battery crate to monitor power
[ ] Implement throttling policy
[ ] Test: Simulate high load, verify Throttle behavior
```

### 7.3 MVP Acceptance Criteria (End of Phase 5)

```yaml
Functional Acceptance:
  - [ ] Aleph auto-starts on macOS login
  - [ ] Detects "Coding" activity within 30s of opening VS Code
  - [ ] Restores last UserContext after restart
  - [ ] Reconciliation sends "Ready to continue?" notification
  - [ ] At least one High Risk operation implemented (requires user confirmation)

Performance Metrics:
  - [ ] Background CPU usage < 5%
  - [ ] Resident memory < 100MB
  - [ ] Event processing latency < 500ms
  - [ ] Cold start to "Active" state < 10 seconds

User Experience:
  - [ ] No Dock icon, menu bar only
  - [ ] Notifications < 3 per hour
  - [ ] All proactive operations reversible or refusable
```

### 7.4 Technology Stack

```toml
# Cargo.toml additions
[dependencies]
# Daemon
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Watchers
sysinfo = "0.30"           # Process monitoring
notify = "6"               # File system monitoring
battery = "0.7"            # Battery status
user-idle = "0.5"          # User idle detection

# IPC
tokio-tungstenite = "0.21" # Or use tarpc

# Notifications (macOS)
mac-notification-sys = "0.6"

# Service Management
plist = "1"                # Generate launchd plist
```

### 7.5 Risks and Mitigation

| Risk | Mitigation Strategy |
|------|---------------------|
| High-frequency events causing CPU spike | Implement backpressure in EventBus, add debouncing to Watchers |
| User privacy concerns | Transparency: TUI Dashboard shows all monitoring, one-click disable |
| Notification overload | Rate limiting in PolicyEngine |
| State inference errors | Use simple rules initially, log all inference for debugging |

---

## 8. Success Metrics

### 8.1 Technical Metrics

- **Latency:** Event-to-action < 1 second
- **Reliability:** Uptime > 99.9% (with auto-recovery)
- **Efficiency:** CPU < 5%, RAM < 100MB

### 8.2 User Experience Metrics

- **Invisibility:** Notifications < 3 per hour
- **Accuracy:** Context detection accuracy > 90%
- **Trust:** User approval rate for High Risk actions > 80%

### 8.3 Qualitative Goals

- User perception: "Aleph knows what I'm doing"
- User behavior: Starts relying on proactive suggestions
- User feedback: "It feels like having an assistant"

---

## 9. Future Extensions

### 9.1 Phase 6: Learning Mode

- Implement `LearnFromHistory` policy
- Track user approval patterns
- Auto-downgrade risk levels based on consistency

### 9.2 Phase 7: Advanced Watchers

- **CalendarWatcher:** Integrate with macOS Calendar
- **EmailWatcher:** Monitor inbox (with explicit permission)
- **GitWatcher:** Detect branch switches, merge conflicts

### 9.3 Phase 8: Multi-Agent Collaboration

- Different agents for different contexts (DevAgent, MeetingAgent, etc.)
- Context-aware agent switching
- Cross-agent memory sharing

---

## 10. Conclusion

This design transforms Aleph from a reactive tool into a proactive companion by:

1. **Layered Architecture** — Clear separation between sensing, understanding, and acting
2. **Smart Tiering** — "Predict, not nag" through risk-based execution strategies
3. **Cognitive Continuity** — Stateful across restarts, learning from user patterns
4. **Safety First** — Every proactive action is reversible or requires confirmation

The result is a system that embodies the JARVIS aesthetic: invisible, intelligent, and indispensable.

---

**Design Status:** ✅ Approved for Implementation
**Next Step:** Create isolated workspace using `superpowers:using-git-worktrees` and detailed implementation plan using `superpowers:writing-plans`
