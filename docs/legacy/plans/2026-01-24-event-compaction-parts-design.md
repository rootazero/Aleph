# Event Bus, Smart Compaction & Message Parts Enhancement Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph's event system, compaction strategy, and message structure to match OpenCode's capabilities.

**Architecture:** Three-layer enhancement - GlobalBus for cross-agent events, Smart Compaction for intelligent context management, and enriched SessionPart types for detailed execution tracking.

**Tech Stack:** Rust (tokio broadcast, serde), UniFFI for Swift bindings

---

## Background

Based on analysis of OpenCode's implementation, Aleph needs enhancements in:

1. **Event Bus** - Add GlobalBus for cross-agent communication + FFI subscription API
2. **Smart Compaction** - Intelligent tool output truncation, turn protection, token budget management
3. **Message Parts** - Richer part types for step boundaries, snapshots, patches, streaming

Priority order: Event Bus → Smart Compaction → Message Parts → (Session Share/Revert deferred)

---

## Part 1: Event Bus Enhancement Architecture

### Overall Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Swift UI Layer                          │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
│  │ SessionView     │  │ ToolProgressView│  │ GlobalMonitor   │ │
│  │ (subscribe      │  │ (subscribe      │  │ (subscribe All) │ │
│  │  session-1)     │  │  ToolCall*)     │  │                 │ │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘ │
└───────────┼────────────────────┼────────────────────┼───────────┘
            │                    │                    │
            ▼                    ▼                    ▼
┌─────────────────────────────────────────────────────────────────┐
│                    EventSubscriptionManager (FFI)               │
│  subscribe(session_id?, event_types) → subscription_id         │
│  unsubscribe(subscription_id)                                   │
└─────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│                         GlobalBus                                │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐                        │
│  │ Agent-1  │ │ Agent-2  │ │ Agent-3  │  (Multi-Agent)         │
│  │ EventBus │ │ EventBus │ │ EventBus │                        │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘                        │
│       └────────────┴────────────┘                               │
│                    │                                             │
│            broadcast to GlobalBus                                │
└─────────────────────────────────────────────────────────────────┘
```

### Core Components

| Component | Responsibility |
|-----------|----------------|
| **GlobalBus** | Process-level singleton, aggregates all Agent events |
| **EventBus** | Agent instance-level, maintains existing functionality |
| **EventSubscriptionManager** | FFI layer subscription management, supports dynamic subscriptions |
| **EventFilter** | Filter by: session_id + event_types + agent_id |

---

## Part 2: GlobalBus Implementation

### GlobalBus Singleton

```rust
// core/src/event/global_bus.rs
pub struct GlobalBus {
    sender: broadcast::Sender<GlobalEvent>,
    subscriptions: RwLock<HashMap<SubscriptionId, Subscription>>,
    agent_buses: RwLock<HashMap<String, Weak<EventBus>>>,
}

#[derive(Clone)]
pub struct GlobalEvent {
    pub source_agent_id: String,
    pub source_session_id: String,
    pub event: AlephEvent,
    pub timestamp: i64,
    pub sequence: u64,
}

pub struct Subscription {
    pub id: SubscriptionId,
    pub filter: EventFilter,
    pub callback: Arc<dyn Fn(GlobalEvent) + Send + Sync>,
}

pub struct EventFilter {
    pub session_ids: Option<HashSet<String>>,  // None = all sessions
    pub agent_ids: Option<HashSet<String>>,    // None = all agents
    pub event_types: Vec<EventType>,           // Required
}
```

### Registration & Broadcasting

```rust
impl GlobalBus {
    /// Agent registers its EventBus
    pub fn register_agent(&self, agent_id: &str, bus: Arc<EventBus>) {
        self.agent_buses.write().insert(agent_id.into(), Arc::downgrade(&bus));
    }

    /// EventBus auto-broadcasts to GlobalBus on publish
    pub async fn broadcast(&self, agent_id: &str, session_id: &str, event: AlephEvent) {
        let global_event = GlobalEvent {
            source_agent_id: agent_id.into(),
            source_session_id: session_id.into(),
            event,
            timestamp: chrono::Utc::now().timestamp_millis(),
            sequence: self.next_sequence(),
        };

        // Notify matching subscribers
        for sub in self.subscriptions.read().values() {
            if sub.filter.matches(&global_event) {
                (sub.callback)(global_event.clone());
            }
        }
    }
}
```

### Use Cases

- **Sub-agent notifies parent**: Parent subscribes to child session's `LoopStop` event
- **UI monitors multiple sessions**: Subscribe to all `SessionCompacted` events for token stats
- **Cross-agent coordination**: One agent completion triggers another agent

---

## Part 3: FFI Subscription Management

### Swift API Design

```swift
// Swift API
class AlephEventSubscription {
    let subscriptionId: String

    static func subscribe(
        sessionId: String? = nil,       // nil = all sessions
        eventTypes: [EventType],        // Required
        handler: @escaping (AetherEvent) -> Void
    ) -> AlephEventSubscription

    func unsubscribe()
}

// Usage Example
let sub = AlephEventSubscription.subscribe(
    sessionId: "session-123",
    eventTypes: [.toolCallStarted, .toolCallCompleted, .loopStop]
) { event in
    switch event {
    case .toolCallCompleted(let result):
        updateToolProgress(result)
    case .loopStop(let reason):
        showSessionEnded(reason)
    default: break
    }
}

// Cleanup
sub.unsubscribe()
```

### Rust FFI Interface

```rust
// core/src/ffi/subscription.rs

/// Create subscription (returns subscription_id)
#[uniffi::export]
pub fn subscribe_events(
    session_id: Option<String>,
    event_types: Vec<String>,  // ["ToolCallStarted", "LoopStop"]
    handler: Arc<dyn EventSubscriptionHandler>,
) -> String;

/// Cancel subscription
#[uniffi::export]
pub fn unsubscribe_events(subscription_id: String);

/// Subscription callback trait
#[uniffi::export(callback_interface)]
pub trait EventSubscriptionHandler: Send + Sync {
    fn on_event(&self, event_json: String);  // JSON serialized event
}
```

### Compile-time vs Runtime

| Mode | Use Case | Implementation |
|------|----------|----------------|
| **Compile-time** | Basic events (session start/stop) | Existing `AlephEventHandler` trait methods |
| **Runtime** | Dynamic subscriptions (specific session's tool events) | `subscribe_events()` API |

---

## Part 4: Smart Compaction Strategy

### Strategy Architecture

```rust
// core/src/compressor/smart_strategy.rs

pub struct SmartCompactionStrategy {
    /// Max chars to retain for tool output
    pub tool_output_max_chars: usize,        // Default: 2000
    /// Protect recent N conversation turns
    pub protected_turns: usize,               // Default: 2
    /// Token budget threshold (0.0-1.0)
    pub compaction_threshold: f32,            // Default: 0.85
    /// Never compact these tool types
    pub protected_tools: HashSet<String>,     // ["skill", "plan"]
}

pub enum CompactionAction {
    /// Keep as-is
    Keep,
    /// Truncate output, retain summary
    Truncate { max_chars: usize, summary: String },
    /// Remove output entirely
    RemoveOutput,
    /// Merge multiple parts into summary
    Summarize { original_count: usize },
}
```

### Compaction Decision Flow

```
┌─────────────────────────────────────────────────────────────┐
│                    SmartCompactor                           │
├─────────────────────────────────────────────────────────────┤
│  1. Check Token Budget                                      │
│     └─ Under threshold → No compaction                      │
│     └─ Over threshold → Continue evaluation                 │
│                                                             │
│  2. Identify Protected Content                              │
│     └─ Recent N turns → Keep                                │
│     └─ protected_tools list → Keep                          │
│                                                             │
│  3. Process Tool Outputs                                    │
│     └─ Output > max_chars → Truncate + generate summary    │
│     └─ Old tool calls → RemoveOutput (keep call record)    │
│                                                             │
│  4. Generate CompactionMarker                               │
│     └─ Record compaction boundary and freed tokens          │
└─────────────────────────────────────────────────────────────┘
```

### Tool Output Truncation Example

```rust
// Before: 10KB file content
ToolCallPart {
    tool_name: "read_file",
    output: Some("// 10000 chars of code..."),
}

// After: Truncated + summary
ToolCallPart {
    tool_name: "read_file",
    output: Some("[Truncated: 10KB → 500 chars] // First 500 chars..."),
    compacted_at: Some(1706123456),
}
```

---

## Part 5: Message Parts Enrichment

### New SessionPart Types

```rust
// core/src/components/types.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionPart {
    // === Existing Types ===
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    Summary(SummaryPart),

    // === New Types ===

    /// Step boundary - start
    StepStart(StepStartPart),
    /// Step boundary - finish
    StepFinish(StepFinishPart),
    /// Filesystem snapshot
    Snapshot(SnapshotPart),
    /// File change record
    Patch(PatchPart),
    /// Compaction boundary marker
    CompactionMarker(CompactionMarkerPart),
    /// Incremental streaming text
    StreamingText(StreamingTextPart),
}
```

### Type Definitions

```rust
/// Step start marker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepStartPart {
    pub step_id: usize,
    pub timestamp: i64,
    pub snapshot_id: Option<String>,  // Associated file snapshot
}

/// Step finish marker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFinishPart {
    pub step_id: usize,
    pub reason: StepFinishReason,  // Completed, Failed, UserAborted
    pub tokens: TokenUsage,
    pub duration_ms: u64,
}

/// Filesystem snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotPart {
    pub snapshot_id: String,
    pub files: Vec<FileSnapshot>,  // File path + hash
    pub timestamp: i64,
}

/// File changes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPart {
    pub patch_id: String,
    pub base_snapshot_id: String,
    pub changes: Vec<FileChange>,  // Added, Modified, Deleted
}

/// Compaction marker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMarkerPart {
    pub marker_id: String,
    pub parts_compacted: usize,      // Number of compacted parts
    pub tokens_freed: u64,           // Freed token count
    pub auto: bool,                  // Auto vs manual trigger
    pub timestamp: i64,
}

/// Streaming text (supports incremental updates)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingTextPart {
    pub part_id: String,
    pub content: String,             // Current full content
    pub is_complete: bool,           // Whether streaming ended
    pub delta: Option<String>,       // Incremental content (for event push)
}
```

---

## Part 6: Implementation Plan

### File Structure Changes

```
core/src/
├── event/
│   ├── mod.rs
│   ├── bus.rs              # Existing EventBus
│   ├── global_bus.rs       # [NEW] GlobalBus singleton
│   ├── subscription.rs     # [NEW] Subscription management
│   └── filter.rs           # [NEW] EventFilter
│
├── compressor/
│   ├── mod.rs
│   ├── smart_strategy.rs   # [NEW] Smart Compaction strategy
│   ├── tool_truncator.rs   # [NEW] Tool output truncation
│   └── turn_protector.rs   # [NEW] Conversation turn protection
│
├── components/
│   └── types.rs            # [MODIFY] New SessionPart types
│
└── ffi/
    ├── mod.rs              # [MODIFY] Add subscription API
    └── subscription.rs     # [NEW] FFI subscription interface
```

### Implementation Phases

| Phase | Content | Est. Files |
|-------|---------|------------|
| **Phase 1** | GlobalBus + EventFilter | 3 |
| **Phase 2** | FFI Subscription Management | 2 |
| **Phase 3** | Smart Compaction Strategy | 3 |
| **Phase 4** | Message Parts Extension | 2 |
| **Phase 5** | Integration Tests + Docs | 2 |

### Dependencies

```
Phase 1 (GlobalBus)
    │
    ├──► Phase 2 (FFI Subscription)
    │
    └──► Phase 3 (Smart Compaction) ──► Phase 4 (Parts)
                                              │
                                              ▼
                                        Phase 5 (Integration)
```

---

## Deferred: Session Share/Revert

Session share and revert functionality is deferred to a future iteration. The Snapshot and Patch parts added in Phase 4 lay the groundwork for this feature.

---

## Success Criteria

1. **Event Bus**: Swift UI can dynamically subscribe to specific session events
2. **GlobalBus**: Sub-agent events visible to parent agent
3. **Smart Compaction**: Tool outputs auto-truncated when approaching token limit
4. **Protected Turns**: Last 2 conversation turns never compacted
5. **Message Parts**: UI can render step boundaries and streaming text with deltas
