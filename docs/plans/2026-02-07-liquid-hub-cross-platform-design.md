# Aleph Liquid Hub - Cross-Platform Browser Control & Host Abstraction Layer

**Date**: 2026-02-07  
**Status**: Design  
**Authors**: Architecture Team  

## Table of Contents

1. [Overview](#overview)
2. [Architecture Principles](#architecture-principles)
3. [Core Components](#core-components)
   - 3.1 [BrowserPool](#31-browserpool---adaptive-process-manager)
   - 3.2 [ContextRegistry](#32-contextregistry---session-lifecycle-manager)
   - 3.3 [TaskScheduler](#33-taskscheduler---intelligent-priority-based-scheduler)
   - 3.4 [LiquidStream](#34-liquidstream---real-time-visual-feedback-service)
   - 3.5 [Aleph HAL](#35-aleph-hal---hardware-abstraction-layer)
4. [Decision Matrix](#decision-matrix---intelligent-routing-rules)
5. [Data Flow](#data-flow---end-to-end-interaction)
6. [Implementation Roadmap](#implementation-roadmap)
7. [Comparison with OpenClaw](#comparison-with-openclaw)

---

## Overview

### Project Name
Aleph Liquid Hub - Cross-Platform Browser Control & Host Abstraction Layer

### Vision
Transform Aleph from a desktop assistant into a true Personal AI Hub, where the Server acts as a persistent digital brain capable of operating browsers and controlling host systems across platforms, while thin clients (iPhone, macOS, Windows, Linux) serve merely as interaction portals.

### Core Problem
Current implementation has two critical limitations:

1. **Browser Control**: Existing \`BrowserService\` is single-session, tightly coupled to local execution, unsuitable for multi-tenant Hub scenarios
2. **Platform Lock-in**: Heavy reliance on macOS Accessibility API prevents cross-platform deployment

### Solution
Introduce a three-layer architecture:

- **BrowserPool**: Adaptive multi-context browser management with persistent user sessions
- **Aleph HAL**: Hardware Abstraction Layer unifying CDP (browser) and native UI control (host) across platforms
- **Intelligent Scheduler**: Priority-based task routing with automatic conflict resolution

### Key Innovation
Unlike OpenClaw's stateless, Node.js-based approach, Aleph implements a **stateful, Rust-native Hub** that maintains persistent browser contexts, supports hot recovery after restarts, and provides real-time visual streaming to thin clients.

### Target Scenario
User on iPhone sends "book a flight to Tokyo" → Hub Server (running on NAS/Mac/Cloud) operates headless browser with user's logged-in session → Streams visual feedback to iPhone → User can intervene anytime with absolute priority.

---

## Architecture Principles

### 1. The Puppeteer Model (傀儡师模型)

Client devices are "prosthetic bodies" with zero business logic. They only perform two functions:

- **Input Conduction**: Transmit user actions (hotkeys, clipboard changes, window focus) and raw sensory data (screenshots, accessibility trees) to Server without any filtering or interpretation
- **Output Execution**: Unconditionally execute atomic commands from Server (render UI, click coordinates, simulate keystrokes)

This ensures the Server (Hub) remains the single source of truth for all decision-making, enabling true cross-platform consistency.

### 2. User-Override Principle (用户主权原则)

User's current intent has absolute execution priority. The system implements a three-tier preemptive priority model:

- **Tier 0**: Real-time user actions (immediate preemption, all background tasks suspended)
- **Tier 1**: User requests (high-priority queue insertion)
- **Tier 2**: Background routines (opportunistic execution during idle periods)

When conflicts occur, the system uses **State Freeze** rather than violent termination, preserving task progress for later resumption.

### 3. Adaptive Execution (自适应执行)

Both process management and concurrency control adapt to context:

- **Resource-aware**: Single-instance mode on NAS, multi-instance on powerful servers
- **Risk-aware**: Financial operations use exclusive queues, information queries use multi-tab concurrency
- **Learning-capable**: System records which websites cause conflicts and automatically adjusts strategy

### 4. Full State Persistence (全状态持久化)

Primary Context maintains complete browser state (Cookies, LocalStorage, IndexedDB) across Server restarts, enabling the Hub to act as a true "digital twin" that never forgets user's logged-in sessions.

---

## Core Components

### 3.1 BrowserPool - Adaptive Process Manager

**Purpose**: Manage browser instances and contexts with adaptive resource allocation.

**Architecture**:
```rust
struct BrowserPool {
    // Primary instance (persistent user context)
    primary_instance: Browser,
    primary_context: BrowserContext,
    
    // Shared instance pool (for normal tasks)
    shared_instances: Vec<Browser>,
    
    // Dedicated instances (for high-risk tasks)
    dedicated_instances: HashMap<TaskId, Browser>,
    
    // Resource monitor
    resource_monitor: ResourceMonitor,
    
    // Configuration
    allocation_policy: AllocationPolicy,
}

enum AllocationPolicy {
    SingleInstance,   // All contexts share one process
    MultiInstance,    // Each context gets dedicated process
    Adaptive,         // Auto-decide based on resources
}
```

**Key Features**:
- **Hot Recovery**: Automatically loads \`user-data-dir\` on Server restart, restoring all logged-in sessions
- **Resource Monitoring**: Tracks system load and available RAM to dynamically switch between Single/Multi-Instance modes
- **Fault Isolation**: If primary instance crashes, automatically restarts and attempts to restore last known state
- **Encryption Support**: \`user-data-dir\` can be stored on encrypted volumes or protected by OS keychain

### 3.2 ContextRegistry - Session Lifecycle Manager

**Purpose**: Manage the mapping between tasks and browser contexts, implementing the Hybrid isolation strategy.

**Architecture**:
```rust
struct ContextRegistry {
    // Primary persistent context (user's digital identity)
    primary_context: ContextHandle,
    
    // Ephemeral contexts (task-specific isolation)
    ephemeral_contexts: HashMap<TaskId, ContextHandle>,
    
    // Domain-based locking (prevent same-domain conflicts)
    domain_locks: HashMap<String, TaskId>,
    
    // Context metadata
    context_metadata: HashMap<ContextId, ContextMetadata>,
}

struct ContextMetadata {
    creation_time: Timestamp,
    last_access: Timestamp,
    isolation_level: IsolationLevel,
    persistent: bool,
    user_data_dir: Option<PathBuf>,
}
```

**Key Features**:
- **Persistent Primary Context**: Maintains user's logged-in state across Server restarts (Cookies + LocalStorage + IndexedDB)
- **Automatic Cleanup**: Ephemeral contexts are destroyed when tasks complete or timeout
- **Domain Locking**: Prevents concurrent tasks from operating on the same domain (e.g., two tasks trying to use the same bank account)
- **Context Snapshots**: Periodically saves context state for recovery after crashes

---

### 3.3 TaskScheduler - Intelligent Priority-Based Scheduler

**Purpose**: Implement the User-Override Principle with intelligent routing and conflict resolution.

**Architecture**:
```rust
struct TaskScheduler {
    // Priority queues for three tiers
    tier0_queue: VecDeque<Task>,  // Real-time user actions
    tier1_queue: VecDeque<Task>,  // User requests
    tier2_queue: VecDeque<Task>,  // Background routines
    
    // Active task tracking
    active_tasks: HashMap<TaskId, TaskState>,
    
    // Domain-based conflict detection
    domain_locks: HashMap<String, TaskId>,
    
    // Decision matrix
    routing_matrix: RoutingMatrix,
}

struct RoutingMatrix {
    // Maps (task_type, domain_type, risk_level) -> ConcurrencyPolicy
    rules: Vec<RoutingRule>,
}

enum ConcurrencyPolicy {
    Exclusive,      // Queue, no concurrency
    SharedTab,      // Multi-tab within same context
    Isolated,       // Dedicated context/process
    Interruptible,  // Can be suspended by higher priority
}
```

**Key Features**:
- **Preemptive Scheduling**: Tier 0 tasks immediately suspend all lower-tier tasks using State Freeze (CDP \`Debugger.pause\`)
- **Domain-Based Locking**: Prevents concurrent operations on same domain (e.g., two tasks accessing same bank account)
- **Automatic Learning**: Records which websites cause conflicts and adjusts routing rules
- **Graceful Degradation**: Under resource pressure, automatically serializes tasks that would normally run concurrently

**Decision Matrix Example**:
```
Task: "Transfer money" + Domain: "bank.com" + Risk: High
  → Policy: Exclusive (queue all other tasks)

Task: "Check weather" + Domain: "weather.com" + Risk: Low
  → Policy: SharedTab (allow concurrent execution)

Task: "Monitor price" + Domain: any + Risk: Low
  → Policy: Isolated (dedicated context for long-running)
```

### 3.4 LiquidStream - Real-Time Visual Feedback Service

**Purpose**: Stream browser visual state to thin clients (iPhone, etc.) for transparent monitoring and intervention.

**Architecture**:
```rust
struct LiquidStream {
    // Active streams per client
    streams: HashMap<ClientId, StreamHandle>,
    
    // Frame capture configuration
    capture_config: CaptureConfig,
    
    // Compression pipeline
    encoder: VideoEncoder,
}

struct CaptureConfig {
    fps: u8,              // 5-30 fps
    quality: Quality,     // Low/Medium/High
    format: Format,       // WebP/H264/VP9
    roi: Option<Rect>,    // Region of interest
}
```

**Key Features**:
- **CDP Screencast**: Uses \`Page.startScreencast\` for efficient frame capture
- **Adaptive Quality**: Automatically adjusts FPS and compression based on network conditions
- **Context Indicators**: Embeds visual markers (gold border for primary context, purple for isolated)
- **Snapshot Aggregator**: Provides iOS-style multi-task switcher showing thumbnails of all active browser contexts
- **Intervention Support**: When user taps on stream, immediately elevates that task to Tier 0 priority

**Data Flow**:
```
Browser Frame → CDP Capture → Compress (WebP) → 
WebSocket Push → iPhone Client → Liquid Glass UI Render
```

---

### 3.5 Aleph HAL - Hardware Abstraction Layer

**Purpose**: Provide unified cross-platform interface for both browser control (CDP) and native host control (Accessibility APIs, input simulation).

**Architecture**:
```rust
// Unified abstraction trait
trait HostProvider: Send + Sync {
    async fn get_active_window(&self) -> Result<WindowInfo>;
    async fn get_accessibility_tree(&self, window: WindowId) -> Result<AccessibilityTree>;
    async fn click(&self, x: i32, y: i32) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn screenshot(&self, region: Option<Rect>) -> Result<Image>;
}

// Platform-specific implementations
struct MacOSProvider {
    // Uses existing AXUIElement APIs
}

struct WindowsProvider {
    // Uses windows-rs + UI Automation
}

struct LinuxProvider {
    // Uses at-spi2-atk + X11/Wayland
}

// Browser provider (cross-platform via CDP)
struct BrowserProvider {
    pool: Arc<BrowserPool>,
}

// Unified HAL facade
struct AlephHAL {
    host_provider: Box<dyn HostProvider>,
    browser_provider: BrowserProvider,
    execution_router: ExecutionRouter,
}
```

**Key Features**:

**1. Dual-Mode Control**:
- **Mode A: Over-the-Shoulder** - Uses native host provider to observe and assist user's current window (preserves existing macOS AX functionality)
- **Mode B: Ghost Worker** - Uses browser provider for autonomous background tasks (headless browser on Server)

**2. Unified Node Abstraction**:
```rust
struct AlephNode {
    id: NodeId,
    role: String,      // "button", "textfield", "link", etc.
    name: String,      // Visible text or aria-label
    bounds: Rect,      // Screen coordinates
    value: Option<String>,
    interactive: bool,
    source: NodeSource,  // Native or Browser
}

enum NodeSource {
    Native(AXElement),
    Browser(ElementRef),
}
```

**3. Server-Client Routing**:
- When Server needs to control Client's local browser (e.g., Safari on macOS), uses ReverseRPC to send commands
- When Server operates its own headless browser, executes directly via CDP
- Routing decision based on \`ExecutionPolicy\` (similar to existing tool routing)

**4. Cross-Platform Input Simulation**:
- Uses \`enigo\` crate for keyboard/mouse simulation
- Abstracts platform differences (X11/Wayland on Linux, CoreGraphics on macOS, Windows API on Windows)
- All input commands are atomic and serializable for RPC transmission

**Integration with Existing Architecture**:
- HAL sits between \`Dispatcher\` and actual execution
- Reuses existing \`ToolRouter\` and \`ExecutionPolicy\` concepts
- Browser operations become just another category of tools, routed intelligently

---

## Decision Matrix - Intelligent Routing Rules

### Routing Decision Factors

```rust
struct RoutingContext {
    task_type: TaskType,
    domain: String,
    risk_level: RiskLevel,
    user_initiated: bool,
    current_load: SystemLoad,
}

enum TaskType {
    Financial,      // Banking, payment, trading
    FormFilling,    // Registration, checkout
    Monitoring,     // Price tracking, news aggregation
    Query,          // Search, information retrieval
    UserInteraction, // Real-time user control
}

enum RiskLevel {
    Low,     // Public information, read-only
    Medium,  // Account operations, data modification
    High,    // Financial transactions, sensitive data
}
```

### Routing Rules Table

```
┌──────────────┬─────────────┬────────────┬─────────────────────────────────┐
│ Task Type    │ Risk Level  │ Domain     │ Policy                          │
├──────────────┼─────────────┼────────────┼─────────────────────────────────┤
│ Financial    │ High        │ bank.*     │ Exclusive + Dedicated Process   │
│ Financial    │ High        │ payment.*  │ Exclusive + Dedicated Process   │
│ FormFilling  │ Medium      │ *.com      │ SharedTab + Domain Lock         │
│ Monitoring   │ Low         │ any        │ Isolated + Background Priority  │
│ Query        │ Low         │ any        │ SharedTab + No Lock             │
│ UserInteract │ any         │ any        │ Tier 0 + Immediate Preemption   │
└──────────────┴─────────────┴────────────┴─────────────────────────────────┘
```

### Conflict Resolution Logic

**Scenario 1: Same Domain Conflict**
```
Task A (running): Filling form on amazon.com
Task B (incoming): Query product on amazon.com

Decision:
- Check domain lock: amazon.com locked by Task A
- Check Task B risk: Low (query only)
- Action: Create new tab in same context, allow concurrent execution
- Rationale: Read operations don't conflict with write operations
```

**Scenario 2: High-Risk Conflict**
```
Task A (running): Transfer money on bank.com
Task B (incoming): Check balance on bank.com

Decision:
- Check domain lock: bank.com locked by Task A
- Check Task A risk: High (financial)
- Action: Queue Task B, wait for Task A completion
- Rationale: Banking sites often detect concurrent sessions and force logout
```

**Scenario 3: User Intervention**
```
Task A (running): Background price monitoring (Tier 2)
User Action (incoming): "Show me this product page" (Tier 0)

Decision:
- Check priority: Tier 0 > Tier 2
- Action: State Freeze Task A, allocate resources to User Action
- Recovery: After user action completes, resume Task A from frozen state
```

### Adaptive Learning

The system maintains a \`ConflictHistory\` database:
```rust
struct ConflictHistory {
    domain: String,
    conflict_count: u32,
    last_conflict: Timestamp,
    recommended_policy: ConcurrencyPolicy,
}
```

**Learning Process**:
1. When a task fails due to concurrent access (detected via error messages or visual anomalies)
2. System records the domain and conflict type
3. After 3+ conflicts on same domain, automatically upgrades to stricter policy
4. Example: \`amazon.com\` initially allows multi-tab, but after detecting session conflicts, switches to domain-locked mode

---

## Data Flow - End-to-End Interaction

### User Request Flow (iPhone → Hub → Browser)

```
┌─────────────────────────────────────────────────────────────────┐
│ iPhone Client (Thin)                                            │
│  - User types: "Book flight to Tokyo"                           │
│  - Sends JSON-RPC: { method: "chat.send", text: "..." }        │
└────────────────────────────┬────────────────────────────────────┘
                             │ WebSocket
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Gateway Layer (Server)                                          │
│  - Routes to Agent Loop                                         │
│  - Session Key: user_123                                        │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Agent Loop (Observe-Think-Act)                                  │
│  - Thinker: Analyzes intent → "book_flight" task               │
│  - Dispatcher: Creates TaskGraph with steps                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ TaskScheduler                                                   │
│  - Classifies: TaskType=FormFilling, Risk=Medium               │
│  - Routing Decision: SharedTab + Primary Context               │
│  - Priority: Tier 1 (User Request)                             │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ BrowserPool                                                     │
│  - Allocates: Primary Context (user's logged-in session)       │
│  - Creates: New Tab in existing browser instance               │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Browser (CDP)                                                   │
│  - Navigate to airline.com                                      │
│  - Fill form fields (departure, destination, dates)            │
│  - Click search button                                          │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ LiquidStream                                                    │
│  - Captures frames via Page.startScreencast                     │
│  - Compresses to WebP                                           │
│  - Pushes to iPhone via WebSocket                              │
└────────────────────────────┬────────────────────────────────────┘
                             │ WebSocket
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ iPhone Client                                                   │
│  - Renders in Liquid Glass UI                                   │
│  - Shows gold border (Primary Context indicator)                │
│  - User sees AI filling the form in real-time                   │
└─────────────────────────────────────────────────────────────────┘
```

### User Intervention Flow (Tier 0 Preemption)

```
Background: Task A (price monitoring) running in Tier 2

User Action: Taps on iPhone screen → "Show me this product"
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ TaskScheduler                                                   │
│  - Detects: Tier 0 priority (Real-time User Action)            │
│  - Action: State Freeze Task A                                 │
│    → Sends CDP: Debugger.pause to Task A's tab                 │
│    → Saves Task A state: { url, scroll_position, step: 3/10 }  │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ BrowserPool                                                     │
│  - Switches focus to new tab for User Action                   │
│  - Allocates full resources to Tier 0 task                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ User Action Completes                                           │
│  - TaskScheduler evaluates: Can Task A resume?                 │
│  - If yes: Sends CDP: Debugger.resume                          │
│  - If no (context changed): Marks Task A as obsolete           │
└─────────────────────────────────────────────────────────────────┘
```

---

### Persistent Context Recovery Flow (Server Restart)

```
Server Shutdown → user-data-dir saved to disk
                             │
                             ↓
Server Restart
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ BrowserPool Initialization                                      │
│  - Scans user-data-dir: /var/aleph/browser/user_123            │
│  - Finds: Cookies, LocalStorage, IndexedDB                     │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Primary Context Restoration                                     │
│  - Launches browser with --user-data-dir flag                  │
│  - All logged-in sessions automatically restored               │
│  - User's digital identity persists across restarts            │
└─────────────────────────────────────────────────────────────────┘
```

### Cross-Platform HAL Routing

```
Dispatcher: "Click button on current window"
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Aleph HAL                                                       │
│  - Detects: Window is Safari (native browser)                  │
│  - Decision: Use Native Provider (macOS AX)                    │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ MacOSProvider                                                   │
│  - Queries AXUIElement tree                                     │
│  - Finds button element                                         │
│  - Simulates click via AXPress                                  │
└─────────────────────────────────────────────────────────────────┘

vs.

Dispatcher: "Click button on headless browser task"
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ Aleph HAL                                                       │
│  - Detects: Task uses BrowserPool context                      │
│  - Decision: Use Browser Provider (CDP)                        │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ↓
┌─────────────────────────────────────────────────────────────────┐
│ BrowserProvider                                                 │
│  - Resolves element via CSS selector                           │
│  - Sends CDP: DOM.click                                         │
└─────────────────────────────────────────────────────────────────┘
```

---

## Implementation Roadmap

### Phase 1: Foundation - BrowserPool & ContextRegistry (2-3 weeks)

**Goal**: Establish multi-context browser management with persistent sessions.

**Deliverables**:
1. **BrowserPool Implementation**
   - Refactor existing \`BrowserService\` into \`BrowserPool\`
   - Add \`AllocationPolicy\` enum (SingleInstance/MultiInstance/Adaptive)
   - Implement \`ResourceMonitor\` for system load tracking
   - Add hot recovery logic for \`user-data-dir\` restoration

2. **ContextRegistry Implementation**
   - Create \`ContextHandle\` abstraction over CDP \`BrowserContext\`
   - Implement Primary Context with persistent storage
   - Add Ephemeral Context lifecycle management
   - Implement domain-based locking mechanism

3. **Testing**
   - Unit tests for context creation/destruction
   - Integration tests for concurrent context operations
   - Persistence tests (restart Server, verify session recovery)

**Success Criteria**:
- Server can maintain 5+ concurrent browser contexts
- Primary Context survives Server restart with all cookies intact
- Domain locks prevent concurrent operations on same domain

---

### Phase 2: Intelligent Scheduling (2 weeks)

**Goal**: Implement priority-based task scheduling with conflict resolution.

**Deliverables**:
1. **TaskScheduler Implementation**
   - Three-tier priority queue system
   - Routing matrix with initial rule set
   - State Freeze mechanism using CDP \`Debugger.pause\`
   - Conflict detection and resolution logic

2. **Integration with Dispatcher**
   - Modify existing \`Dispatcher\` to route browser tasks through \`TaskScheduler\`
   - Add task metadata (type, risk_level, priority)
   - Implement preemption logic for Tier 0 tasks

3. **Testing**
   - Concurrency tests (multiple tasks on same domain)
   - Preemption tests (Tier 0 interrupts Tier 2)
   - Recovery tests (resume frozen tasks)

**Success Criteria**:
- User actions immediately preempt background tasks
- Financial operations automatically queue (no concurrency)
- System learns from conflicts and adjusts routing rules

---

### Phase 3: Visual Streaming (1-2 weeks)

**Goal**: Enable real-time visual feedback to thin clients.

**Deliverables**:
1. **LiquidStream Implementation**
   - CDP \`Page.startScreencast\` integration
   - WebP/H264 compression pipeline
   - WebSocket streaming to clients
   - Context indicator overlay (gold/purple borders)

2. **Client Integration**
   - Modify iPhone/macOS client to receive and render streams
   - Add Liquid Glass UI for stream display
   - Implement tap-to-intervene (elevate task to Tier 0)

3. **SnapshotAggregator**
   - Multi-context thumbnail generation
   - iOS-style task switcher UI

**Success Criteria**:
- User on iPhone sees real-time browser operations (<500ms latency)
- Visual indicators clearly distinguish Primary vs Isolated contexts
- User can tap stream to take control (immediate preemption)

---

### Phase 4: Cross-Platform HAL (3-4 weeks)

**Goal**: Abstract platform-specific APIs into unified interface.

**Deliverables**:
1. **HAL Trait Definition**
   - Define \`HostProvider\` trait
   - Create \`AlephNode\` unified abstraction
   - Implement \`ExecutionRouter\` for Server/Client routing

2. **Platform Providers**
   - \`MacOSProvider\`: Wrap existing AX APIs
   - \`WindowsProvider\`: Implement using \`windows-rs\` + UI Automation
   - \`LinuxProvider\`: Implement using \`at-spi2-atk\`
   - \`BrowserProvider\`: Wrap \`BrowserPool\` operations

3. **Input Simulation**
   - Integrate \`enigo\` for cross-platform keyboard/mouse
   - Implement atomic command serialization for RPC

4. **Testing**
   - Cross-platform compatibility tests
   - Server-Client routing tests
   - Native vs Browser provider switching tests

**Success Criteria**:
- Same Rust code controls browsers on macOS/Windows/Linux
- Server can seamlessly switch between native and browser control
- Client can execute Server's input commands via RPC

---

### Phase 5: Adaptive Learning & Optimization (2 weeks)

**Goal**: Add intelligence and self-optimization capabilities.

**Deliverables**:
1. **ConflictHistory Database**
   - Track domain-specific conflicts
   - Automatic policy upgrade after repeated failures
   - Persistent learning across Server restarts

2. **Resource-Aware Adaptation**
   - Dynamic switching between Single/Multi-Instance based on load
   - Automatic task serialization under memory pressure
   - Graceful degradation strategies

3. **Performance Monitoring**
   - Task execution metrics
   - Context switch overhead tracking
   - Visual stream quality adaptation

**Success Criteria**:
- System automatically learns problematic domains (e.g., banking sites)
- Resource allocation adapts to available RAM/CPU
- Performance metrics guide optimization decisions

---

### Phase 6: Security & Encryption (1 week)

**Goal**: Harden security for sensitive data.

**Deliverables**:
1. **Encrypted Storage**
   - Support for encrypted \`user-data-dir\` volumes
   - OS keychain integration for session tokens
   - Secure deletion of ephemeral contexts

2. **Audit Logging**
   - Log all browser operations for security review
   - Track which tasks accessed which domains
   - Alert on suspicious patterns

**Success Criteria**:
- Primary Context data encrypted at rest
- Audit trail available for compliance review
- No sensitive data leaks in logs or temp files

---

## Comparison with OpenClaw

### Architecture Philosophy

**OpenClaw**:
- **Stateless Execution**: Each skill is a one-shot operation, no persistent state
- **Node.js Ecosystem**: Heavy dependency on Playwright, Electron, npm packages
- **Tool-Centric**: User explicitly invokes skills, AI acts as executor
- **Local-First**: Primarily designed for desktop usage, limited remote capabilities

**Aleph Liquid Hub**:
- **Stateful Agent**: Maintains persistent browser contexts, acts as digital twin
- **Rust-Native**: Direct CDP integration, minimal dependencies, cross-platform
- **Hub-Centric**: Server is autonomous entity, clients are thin portals
- **Remote-First**: Designed for distributed deployment (NAS, cloud, edge)

### Key Differentiators

**1. Persistent Identity**
- **OpenClaw**: User must log in for each task execution
- **Aleph**: Primary Context maintains logged-in state indefinitely, survives restarts
- **Impact**: Enables long-running autonomous tasks (daily check-ins, price monitoring)

**2. Intelligent Concurrency**
- **OpenClaw**: Sequential execution, no conflict management
- **Aleph**: Multi-context with intelligent routing, automatic conflict resolution
- **Impact**: Can handle multiple concurrent tasks without user intervention

**3. User Priority System**
- **OpenClaw**: No priority mechanism, tasks run in order
- **Aleph**: Three-tier preemptive scheduling, user actions always win
- **Impact**: User never waits for background tasks, immediate responsiveness

**4. Visual Transparency**
- **OpenClaw**: Headless execution, user sees only final results
- **Aleph**: Real-time visual streaming with context indicators
- **Impact**: User maintains awareness and control over AI operations

**5. Cross-Platform Abstraction**
- **OpenClaw**: Primarily Windows/macOS via Electron
- **Aleph**: Unified HAL supporting macOS/Windows/Linux with native performance
- **Impact**: Single codebase deploys anywhere, no platform-specific rewrites

**6. Adaptive Resource Management**
- **OpenClaw**: Fixed resource allocation per task
- **Aleph**: Dynamic process/context allocation based on system load
- **Impact**: Efficient operation on resource-constrained devices (NAS, Raspberry Pi)

**7. Learning Capability**
- **OpenClaw**: Static skill definitions, no adaptation
- **Aleph**: Conflict history tracking, automatic policy adjustment
- **Impact**: System improves over time, learns problematic domains

### Use Case Comparison

**Scenario: "Book a flight to Tokyo"**

**OpenClaw Approach**:
1. User invokes \`/book-flight\` skill
2. Playwright launches new browser instance
3. User must provide login credentials
4. Skill fills form and completes booking
5. Browser closes, session lost
6. Next booking requires re-login

**Aleph Approach**:
1. User sends natural language request from iPhone
2. TaskScheduler routes to Primary Context (already logged in)
3. BrowserPool allocates tab in existing session
4. LiquidStream shows real-time progress on iPhone
5. User can intervene anytime (Tier 0 preemption)
6. Session persists for future bookings
7. If user starts another task, intelligent routing prevents conflicts

### Performance Characteristics

```
┌─────────────────────┬──────────────┬─────────────────┐
│ Metric              │ OpenClaw     │ Aleph Liquid    │
├─────────────────────┼──────────────┼─────────────────┤
│ Cold Start          │ 5-10s        │ <1s (hot pool)  │
│ Memory (idle)       │ ~500MB       │ ~200MB          │
│ Memory (5 tasks)    │ ~2.5GB       │ ~400MB (shared) │
│ Concurrent Tasks    │ 1 (serial)   │ 5+ (parallel)   │
│ Session Persistence │ No           │ Yes (infinite)  │
│ User Latency        │ N/A          │ <500ms (Tier 0) │
│ Platform Support    │ Win/Mac      │ Win/Mac/Linux   │
└─────────────────────┴──────────────┴─────────────────┘
```

### When to Use Each

**Use OpenClaw When**:
- One-time automation tasks
- Desktop-only deployment
- Node.js ecosystem preferred
- Stateless execution acceptable

**Use Aleph When**:
- Long-running autonomous agents
- Multi-device access (iPhone, desktop, etc.)
- Resource-constrained environments
- Persistent identity required
- Real-time user intervention needed
- Cross-platform deployment

---

## Conclusion

The Aleph Liquid Hub architecture represents a fundamental shift from tool-based automation to agent-based autonomy. By combining persistent browser contexts, intelligent scheduling, real-time visual feedback, and cross-platform abstraction, Aleph transcends the limitations of traditional automation frameworks like OpenClaw.

The key innovation is the **Hub-centric model**: the Server acts as a persistent digital brain that maintains user identity, learns from experience, and adapts to resource constraints, while thin clients serve as mere interaction portals. This architecture enables true Personal AI Hub scenarios where users can seamlessly control their digital presence across devices and platforms.

**Next Steps**:
1. Review and approve this design document
2. Begin Phase 1 implementation (BrowserPool & ContextRegistry)
3. Set up development environment for cross-platform testing
4. Create tracking issues for each phase in the roadmap

---

**Document Status**: Ready for Review  
**Last Updated**: 2026-02-07  
**Version**: 1.0
