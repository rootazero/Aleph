# Swarm Intelligence Architecture - Agent Loop Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Integrate Swarm Intelligence Architecture into Agent Loop, enabling horizontal agent collaboration through event-driven communication, context injection, and collective memory.

**Architecture:** Builder pattern for flexible component composition, ContextProvider abstraction for extensible context injection, semantic event publishing with Coordinator classification, dynamic tool registration with auto-injection.

**Tech Stack:** Rust, Tokio, Arc/RwLock for thread-safe state, async/await for event handling

**Related Design:** [Swarm Agent Loop Integration Design](2026-02-11-swarm-agent-loop-integration-design.md)

---

## Phase 1: Shadow Monitor Mode (Infrastructure)

**Goal:** Establish event collection and validation mechanism without interfering with existing Agent behavior.

**Success Criteria:**
- EventBus receives events normally
- Event classification correct (Tier 1/2/3)
- No performance regression (<5% latency increase)
- Event statistics visible in logs

---

### Task 1: Create AgentLoopBuilder Structure

**Files:**
- Create: `core/src/agent_loop/builder.rs`
- Modify: `core/src/agent_loop/mod.rs` (add pub use)

**Step 1: Write the failing test**

Create test file first to drive the design:

```rust
// core/src/agent_loop/builder.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_builder_creation() {
        let thinker = Arc::new(MockThinker::new());
        let executor = Arc::new(MockExecutor::new());
        let compressor = Arc::new(MockCompressor::new());

        let builder = AgentLoopBuilder::new(thinker, executor, compressor);

        assert!(builder.swarm_coordinator.is_none());
        assert!(builder.event_bus.is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph/.worktrees/feature/swarm-agent-loop-integration && cargo test --lib --package alephcore agent_loop::builder::tests::test_builder_creation`

Expected: FAIL with "module `builder` not found"

**Step 3: Write minimal Builder structure**

```rust
// core/src/agent_loop/builder.rs

use std::sync::Arc;
use crate::agent_loop::{LoopConfig, ThinkerTrait, ActionExecutor, CompressorTrait};
use crate::agents::swarm::SwarmCoordinator;
use crate::event::EventBus;
use crate::agent_loop::overflow::OverflowDetector;

/// Builder for AgentLoop with optional components
pub struct AgentLoopBuilder<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    config: LoopConfig,

    // Optional components
    event_bus: Option<Arc<EventBus>>,
    overflow_detector: Option<Arc<OverflowDetector>>,
    swarm_coordinator: Option<Arc<SwarmCoordinator>>,
}

impl<T, E, C> AgentLoopBuilder<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    /// Create a new AgentLoopBuilder with required components
    pub fn new(thinker: Arc<T>, executor: Arc<E>, compressor: Arc<C>) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config: LoopConfig::default(),
            event_bus: None,
            overflow_detector: None,
            swarm_coordinator: None,
        }
    }

    /// Set the loop configuration
    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.config = config;
        self
    }

    /// Add EventBus for compaction triggers
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Add OverflowDetector for real-time overflow checking
    pub fn with_overflow_detector(mut self, detector: Arc<OverflowDetector>) -> Self {
        self.overflow_detector = Some(detector);
        self
    }

    /// Add SwarmCoordinator for agent collaboration
    pub fn with_swarm(mut self, coordinator: Arc<SwarmCoordinator>) -> Self {
        self.swarm_coordinator = Some(coordinator);
        self
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib --package alephcore agent_loop::builder::tests::test_builder_creation`

Expected: PASS

**Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/feature/swarm-agent-loop-integration
git add core/src/agent_loop/builder.rs
git commit -m "feat(agent_loop): add AgentLoopBuilder structure

Create Builder pattern for AgentLoop construction:
- Support optional components (EventBus, OverflowDetector, SwarmCoordinator)
- Fluent API with with_* methods
- Test coverage for builder creation

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 2: Implement Builder build() Method

**Files:**
- Modify: `core/src/agent_loop/builder.rs` (add build method)
- Modify: `core/src/agent_loop/agent_loop.rs` (add swarm_coordinator field)

**Step 1: Write the failing test**

```rust
#[test]
fn test_builder_build_basic() {
    let thinker = Arc::new(MockThinker::new());
    let executor = Arc::new(MockExecutor::new());
    let compressor = Arc::new(MockCompressor::new());

    let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
        .build();

    assert!(agent_loop.swarm_coordinator.is_none());
}

#[test]
fn test_builder_build_with_swarm() {
    let thinker = Arc::new(MockThinker::new());
    let executor = Arc::new(MockExecutor::new());
    let compressor = Arc::new(MockCompressor::new());
    let coordinator = Arc::new(SwarmCoordinator::new(SwarmConfig::default()));

    let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
        .with_swarm(coordinator.clone())
        .build();

    assert!(agent_loop.swarm_coordinator.is_some());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib --package alephcore agent_loop::builder::tests::test_builder_build`

Expected: FAIL with "method `build` not found"

**Step 3: Add swarm_coordinator field to AgentLoop**

```rust
// core/src/agent_loop/agent_loop.rs

pub struct AgentLoop<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    pub(crate) config: LoopConfig,
    compaction_trigger: OptionalCompactionTrigger,
    pub(crate) overflow_detector: Option<Arc<OverflowDetector>>,
    // NEW: Add swarm coordinator
    swarm_coordinator: Option<Arc<SwarmCoordinator>>,
}
```

**Step 4: Implement build() method**

```rust
// core/src/agent_loop/builder.rs

use crate::agent_loop::{AgentLoop, OptionalCompactionTrigger};

impl<T, E, C> AgentLoopBuilder<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    // ... existing methods ...

    /// Build the AgentLoop
    pub fn build(self) -> AgentLoop<T, E, C> {
        // Create compaction trigger
        let compaction_trigger = OptionalCompactionTrigger::new(self.event_bus.clone());

        // Build AgentLoop
        AgentLoop {
            thinker: self.thinker,
            executor: self.executor,
            compressor: self.compressor,
            config: self.config,
            compaction_trigger,
            overflow_detector: self.overflow_detector,
            swarm_coordinator: self.swarm_coordinator,
        }
    }
}
```

**Step 5: Run test to verify it passes**

Run: `cargo test --lib --package alephcore agent_loop::builder::tests::test_builder_build`

Expected: PASS

**Step 6: Commit**

```bash
git add core/src/agent_loop/builder.rs core/src/agent_loop/agent_loop.rs
git commit -m "feat(agent_loop): implement Builder build() method

Add build() method to AgentLoopBuilder:
- Construct AgentLoop with all optional components
- Add swarm_coordinator field to AgentLoop
- Test coverage for build with/without swarm

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 3: Define AgentLoopEvent Enum

**Files:**
- Create: `core/src/agent_loop/events.rs`
- Modify: `core/src/agent_loop/mod.rs` (add pub use)

**Step 1: Write the failing test**

```rust
// core/src/agent_loop/events.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_initiated_creation() {
        let event = AgentLoopEvent::ActionInitiated {
            agent_id: "test-agent".to_string(),
            action_type: "tool_call".to_string(),
            target: Some("read_file".to_string()),
        };

        match event {
            AgentLoopEvent::ActionInitiated { agent_id, .. } => {
                assert_eq!(agent_id, "test-agent");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_decision_made_creation() {
        let event = AgentLoopEvent::DecisionMade {
            agent_id: "test-agent".to_string(),
            decision: "refactor Auth module".to_string(),
            affected_files: vec!["src/auth/mod.rs".to_string()],
        };

        match event {
            AgentLoopEvent::DecisionMade { decision, .. } => {
                assert_eq!(decision, "refactor Auth module");
            }
            _ => panic!("Wrong event type"),
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib --package alephcore agent_loop::events::tests`

Expected: FAIL with "module `events` not found"

**Step 3: Define AgentLoopEvent enum**

```rust
// core/src/agent_loop/events.rs

use serde::{Deserialize, Serialize};
use crate::agent_loop::decision::ActionResult;

/// Semantic events published by AgentLoop at key operation points
/// These events are NOT tier-classified - SwarmCoordinator handles classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentLoopEvent {
    /// Tool execution started
    ActionInitiated {
        agent_id: String,
        action_type: String,
        target: Option<String>,  // File path, tool name, etc.
    },

    /// Tool execution completed
    ActionCompleted {
        agent_id: String,
        action_type: String,
        result: ActionResult,
        duration_ms: u64,
    },

    /// Agent made a decision about next action
    DecisionMade {
        agent_id: String,
        decision: String,  // "refactor Auth module", "fix dependency conflict"
        affected_files: Vec<String>,
    },

    /// Agent captured important insight (error, contradiction, discovery)
    InsightCaptured {
        agent_id: String,
        insight: String,
        severity: InsightSeverity,
    },
}

/// Severity level for insights
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InsightSeverity {
    Info,
    Warning,
    Critical,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib --package alephcore agent_loop::events::tests`

Expected: PASS

**Step 5: Export from mod.rs**

```rust
// core/src/agent_loop/mod.rs

pub mod events;
// ... existing modules ...

pub use events::{AgentLoopEvent, InsightSeverity};
```

**Step 6: Commit**

```bash
git add core/src/agent_loop/events.rs core/src/agent_loop/mod.rs
git commit -m "feat(agent_loop): define AgentLoopEvent enum

Add semantic event types for agent collaboration:
- ActionInitiated: Tool execution started
- ActionCompleted: Tool execution finished
- DecisionMade: Agent decision broadcast
- InsightCaptured: Important discoveries/errors

Events are NOT tier-classified (handled by SwarmCoordinator)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 4: Implement Event Publishing in AgentLoop (Shadow Mode)

**Files:**
- Modify: `core/src/agent_loop/agent_loop.rs` (add event publishing)
- Modify: `core/src/agents/swarm/coordinator.rs` (add publish_event method)

**Step 1: Add publish_event method to SwarmCoordinator**

```rust
// core/src/agents/swarm/coordinator.rs

use crate::agent_loop::AgentLoopEvent;

impl SwarmCoordinator {
    /// Publish AgentLoop event (converts to internal event and classifies)
    pub async fn publish_event(&self, event: AgentLoopEvent) {
        use crate::agent_loop::InsightSeverity;
        use crate::agents::swarm::events::*;

        // Convert to internal event and classify by tier
        let swarm_event = match event {
            AgentLoopEvent::ActionInitiated { agent_id, action_type, target } => {
                AgentEvent::Info(InfoEvent::ActionStarted {
                    agent_id,
                    action_type,
                    target,
                    timestamp: chrono::Utc::now(),
                })
            }
            AgentLoopEvent::ActionCompleted { agent_id, action_type, result, duration_ms } => {
                AgentEvent::Important(ImportantEvent::ToolExecuted {
                    agent_id,
                    tool_name: action_type,
                    result: format!("{:?}", result),
                    duration_ms,
                    timestamp: chrono::Utc::now(),
                })
            }
            AgentLoopEvent::DecisionMade { agent_id, decision, affected_files } => {
                AgentEvent::Important(ImportantEvent::DecisionBroadcast {
                    agent_id,
                    decision,
                    affected_files,
                    timestamp: chrono::Utc::now(),
                })
            }
            AgentLoopEvent::InsightCaptured { agent_id, insight, severity } => {
                match severity {
                    InsightSeverity::Critical => {
                        AgentEvent::Critical(CriticalEvent::ErrorDetected {
                            agent_id,
                            error_message: insight,
                            timestamp: chrono::Utc::now(),
                        })
                    }
                    _ => {
                        AgentEvent::Info(InfoEvent::InsightCaptured {
                            agent_id,
                            insight,
                            timestamp: chrono::Utc::now(),
                        })
                    }
                }
            }
        };

        // Publish to bus
        if let Err(e) = self.bus.publish(swarm_event).await {
            tracing::warn!("Failed to publish swarm event: {}", e);
        }
    }
}
```

**Step 2: Add event publishing in AgentLoop::run()**

```rust
// core/src/agent_loop/agent_loop.rs

// In the run() method, add event publishing at key points:

// 1. Before tool execution (ActionInitiated)
callback.on_action_start(&action).await;
if let Some(ref swarm) = self.swarm_coordinator {
    if let Action::ToolCall { tool_name, .. } = &action {
        swarm.publish_event(AgentLoopEvent::ActionInitiated {
            agent_id: state.session_id.clone(),
            action_type: action.action_type(),
            target: Some(tool_name.clone()),
        }).await;
    }
}

// 2. After tool execution (ActionCompleted)
callback.on_action_done(&action, &result).await;
if let Some(ref swarm) = self.swarm_coordinator {
    swarm.publish_event(AgentLoopEvent::ActionCompleted {
        agent_id: state.session_id.clone(),
        action_type: action.action_type(),
        result: result.clone(),
        duration_ms,
    }).await;
}

// 3. After decision (DecisionMade) - Phase 1 priority
if let Decision::UseTool { tool_name, arguments } = &thinking.decision {
    if let Some(ref swarm) = self.swarm_coordinator {
        let affected_files = extract_affected_files(arguments);
        swarm.publish_event(AgentLoopEvent::DecisionMade {
            agent_id: state.session_id.clone(),
            decision: format!("Using {} to accomplish task", tool_name),
            affected_files,
        }).await;
    }
}
```

**Step 3: Add helper function to extract affected files**

```rust
// core/src/agent_loop/agent_loop.rs

/// Extract file paths from tool arguments
fn extract_affected_files(arguments: &serde_json::Value) -> Vec<String> {
    let mut files = Vec::new();

    // Check common argument names for file paths
    if let Some(obj) = arguments.as_object() {
        for key in &["path", "file_path", "file", "files", "target"] {
            if let Some(value) = obj.get(*key) {
                if let Some(s) = value.as_str() {
                    files.push(s.to_string());
                } else if let Some(arr) = value.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            files.push(s.to_string());
                        }
                    }
                }
            }
        }
    }

    files
}
```

**Step 4: Run tests to verify no regressions**

Run: `cargo test --lib --package alephcore agent_loop`

Expected: All existing tests PASS

**Step 5: Add integration test for event publishing**

```rust
// core/src/agent_loop/agent_loop.rs (in tests module)

#[tokio::test]
async fn test_event_publishing_shadow_mode() {
    let coordinator = Arc::new(SwarmCoordinator::new(SwarmConfig::default()));
    let thinker = Arc::new(MockThinker::new());
    let executor = Arc::new(MockExecutor::new());
    let compressor = Arc::new(MockCompressor::new());

    let agent_loop = AgentLoopBuilder::new(thinker, executor, compressor)
        .with_swarm(coordinator.clone())
        .build();

    // Subscribe to events
    let mut rx = coordinator.bus().subscribe_all();

    // Run agent loop (will publish events)
    let run_context = RunContext::new(
        "test request",
        RequestContext::empty(),
        vec![],
        IdentityContext::default(),
    );

    tokio::spawn(async move {
        agent_loop.run(run_context, NoOpLoopCallback).await;
    });

    // Verify events are published
    let event = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rx.recv()
    ).await;

    assert!(event.is_ok(), "Should receive at least one event");
}
```

**Step 6: Commit**

```bash
git add core/src/agent_loop/agent_loop.rs core/src/agents/swarm/coordinator.rs
git commit -m "feat(agent_loop): implement event publishing (shadow mode)

Add event publishing at key AgentLoop points:
- ActionInitiated: Before tool execution
- ActionCompleted: After tool execution
- DecisionMade: After decision (Phase 1 priority)

SwarmCoordinator converts AgentLoopEvent to internal events and classifies by tier.
Shadow mode: Events published but not yet injected into context.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 5: Define ContextProvider Trait

**Files:**
- Create: `core/src/agent_loop/context_provider.rs`
- Modify: `core/src/agent_loop/mod.rs` (add pub use)

**Step 1: Write the failing test**

```rust
// core/src/agent_loop/context_provider.rs

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        context: String,
        priority: i32,
    }

    impl ContextProvider for MockProvider {
        fn get_context(&self) -> Option<String> {
            Some(self.context.clone())
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn name(&self) -> &str {
            "mock_provider"
        }
    }

    #[test]
    fn test_provider_trait() {
        let provider = MockProvider {
            context: "test context".to_string(),
            priority: 100,
        };

        assert_eq!(provider.get_context(), Some("test context".to_string()));
        assert_eq!(provider.priority(), 100);
        assert_eq!(provider.name(), "mock_provider");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib --package alephcore agent_loop::context_provider::tests`

Expected: FAIL with "module `context_provider` not found"

**Step 3: Define ContextProvider trait**

```rust
// core/src/agent_loop/context_provider.rs

/// Context provider abstract interface
///
/// Implementations provide additional context to be injected into
/// the agent's prompt at message building time.
pub trait ContextProvider: Send + Sync {
    /// Get context content to inject
    ///
    /// Returns None if no context should be injected at this time.
    fn get_context(&self) -> Option<String>;

    /// Priority determines position in prompt
    ///
    /// Higher values appear first. Critical context should have
    /// highest priority (e.g., 1000+), normal context around 100.
    fn priority(&self) -> i32;

    /// Provider name for debugging and logging
    fn name(&self) -> &str;
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib --package alephcore agent_loop::context_provider::tests`

Expected: PASS

**Step 5: Export from mod.rs**

```rust
// core/src/agent_loop/mod.rs

pub mod context_provider;
// ... existing modules ...

pub use context_provider::ContextProvider;
```

**Step 6: Commit**

```bash
git add core/src/agent_loop/context_provider.rs core/src/agent_loop/mod.rs
git commit -m "feat(agent_loop): define ContextProvider trait

Add abstract interface for context injection:
- get_context(): Returns optional context string
- priority(): Determines position in prompt (higher = first)
- name(): Provider identifier for debugging

Enables extensible context sources (Swarm, Security, Performance, etc.)

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 6: Implement SwarmContextProvider

**Files:**
- Create: `core/src/agents/swarm/context_provider.rs`
- Modify: `core/src/agents/swarm/mod.rs` (add pub use)

**Step 1: Write the failing test**

```rust
// core/src/agents/swarm/context_provider.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::*;

    #[tokio::test]
    async fn test_swarm_context_provider_empty() {
        let coordinator = Arc::new(SwarmCoordinator::new(SwarmConfig::default()));
        let provider = SwarmContextProvider::new(coordinator.context_injector());

        // No events yet, should return None
        assert_eq!(provider.get_context(), None);
    }

    #[tokio::test]
    async fn test_swarm_context_provider_with_events() {
        let coordinator = Arc::new(SwarmCoordinator::new(SwarmConfig::default()));

        // Publish some events
        coordinator.publish_event(AgentLoopEvent::DecisionMade {
            agent_id: "BEE-01".to_string(),
            decision: "Refactoring Auth module".to_string(),
            affected_files: vec!["src/auth/mod.rs".to_string()],
        }).await;

        let provider = SwarmContextProvider::new(coordinator.context_injector());

        // Should return formatted context
        let context = provider.get_context();
        assert!(context.is_some());
        assert!(context.unwrap().contains("<team_awareness"));
        assert!(context.unwrap().contains("BEE-01"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib --package alephcore swarm::context_provider::tests`

Expected: FAIL with "module `context_provider` not found"

**Step 3: Implement SwarmContextProvider**

```rust
// core/src/agents/swarm/context_provider.rs

use std::sync::Arc;
use crate::agent_loop::ContextProvider;
use crate::agents::swarm::{ContextInjector, Tier};
use crate::agents::swarm::events::ImportantEvent;

/// Swarm context provider for team awareness injection
pub struct SwarmContextProvider {
    context_injector: Arc<ContextInjector>,
}

impl SwarmContextProvider {
    pub fn new(context_injector: Arc<ContextInjector>) -> Self {
        Self { context_injector }
    }

    /// Format events as team communication protocol (XML)
    fn format_team_awareness(&self, events: &[ImportantEvent]) -> String {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let mut xml = format!(
            "<team_awareness timestamp=\"{}\">\n",
            timestamp
        );

        for event in events {
            match event {
                ImportantEvent::ToolExecuted { agent_id, tool_name, result, .. } => {
                    xml.push_str(&format!(
                        "  <agent id=\"{}\" status=\"completed\">\n",
                        agent_id
                    ));
                    xml.push_str(&format!(
                        "    <action>Executed {}</action>\n",
                        tool_name
                    ));
                    // Extract file paths from result if available
                    if let Some(files) = self.extract_files_from_result(result) {
                        xml.push_str(&format!(
                            "    <files>{}</files>\n",
                            files.join(", ")
                        ));
                    }
                    xml.push_str("  </agent>\n");
                }
                ImportantEvent::DecisionBroadcast { agent_id, decision, affected_files, .. } => {
                    xml.push_str(&format!(
                        "  <agent id=\"{}\" status=\"working\">\n",
                        agent_id
                    ));
                    xml.push_str(&format!(
                        "    <action>{}</action>\n",
                        decision
                    ));
                    if !affected_files.is_empty() {
                        xml.push_str(&format!(
                            "    <files>{}</files>\n",
                            affected_files.join(", ")
                        ));
                    }
                    xml.push_str("  </agent>\n");
                }
            }
        }

        xml.push_str("  <summary>Team activity in the last iteration</summary>\n");
        xml.push_str("</team_awareness>");
        xml
    }

    fn extract_files_from_result(&self, result: &str) -> Option<Vec<String>> {
        // Simple heuristic: look for file paths in result
        // TODO: Improve with structured result parsing
        None
    }
}

impl ContextProvider for SwarmContextProvider {
    fn get_context(&self) -> Option<String> {
        // Get Tier 2 (Important) level context
        // Tier 1 (Critical) handled via execution layer interruption
        // Tier 3 (Info) via tool query
        let events = self.context_injector.get_context_for_tier(Tier::Important)?;

        if events.is_empty() {
            return None;
        }

        // Format as team communication protocol
        Some(self.format_team_awareness(&events))
    }

    fn priority(&self) -> i32 {
        100  // High priority, but lower than Critical interruption (1000+)
    }

    fn name(&self) -> &str {
        "swarm_context"
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib --package alephcore swarm::context_provider::tests`

Expected: PASS

**Step 5: Export from mod.rs**

```rust
// core/src/agents/swarm/mod.rs

pub mod context_provider;
// ... existing modules ...

pub use context_provider::SwarmContextProvider;
```

**Step 6: Commit**

```bash
git add core/src/agents/swarm/context_provider.rs core/src/agents/swarm/mod.rs
git commit -m "feat(swarm): implement SwarmContextProvider

Add ContextProvider implementation for Swarm:
- Retrieves Tier 2 (Important) events from ContextInjector
- Formats as XML team communication protocol
- Priority 100 (high but below Critical)

XML format distinguishes team broadcast from agent's own memory.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 7: Integrate ContextProvider into MessageBuilder

**Files:**
- Modify: `core/src/agent_loop/message_builder/builder.rs`

**Step 1: Add context_providers field to MessageBuilder**

```rust
// core/src/agent_loop/message_builder/builder.rs

use crate::agent_loop::ContextProvider;

pub struct MessageBuilder {
    config: MessageBuilderConfig,
    context_providers: Vec<Box<dyn ContextProvider>>,
    // ... existing fields ...
}

impl MessageBuilder {
    pub fn new(config: MessageBuilderConfig) -> Self {
        Self {
            config,
            context_providers: Vec::new(),
            // ... existing fields ...
        }
    }

    /// Add context providers
    pub fn with_providers(mut self, providers: Vec<Box<dyn ContextProvider>>) -> Self {
        self.context_providers = providers;
        self
    }
}
```

**Step 2: Inject context in build() method**

```rust
// core/src/agent_loop/message_builder/builder.rs

impl MessageBuilder {
    pub fn build(&self, state: &LoopState, tools: &[Tool]) -> Vec<Message> {
        let mut messages = vec![
            self.build_system_message(),
            // ... other messages ...
        ];

        // Inject all ContextProvider contexts (sorted by priority)
        let mut providers = self.context_providers.clone();
        providers.sort_by_key(|p| -p.priority());  // Descending order

        for provider in providers {
            if let Some(context) = provider.get_context() {
                tracing::debug!(
                    provider = provider.name(),
                    "Injecting context from provider"
                );
                messages.push(Message::system(context));
            }
        }

        messages
    }
}
```

**Step 3: Update Builder to pass providers to MessageBuilder**

```rust
// core/src/agent_loop/builder.rs

impl<T, E, C> AgentLoopBuilder<T, E, C> {
    pub fn build(self) -> AgentLoop<T, E, C> {
        let mut context_providers: Vec<Box<dyn ContextProvider>> = Vec::new();

        // If Swarm enabled, add SwarmContextProvider
        if let Some(ref coordinator) = self.swarm_coordinator {
            let provider = SwarmContextProvider::new(
                coordinator.context_injector()
            );
            context_providers.push(Box::new(provider));
        }

        // Build MessageBuilder with providers
        let message_builder = MessageBuilder::new(self.config.message_builder_config())
            .with_providers(context_providers);

        // ... rest of build logic ...
    }
}
```

**Step 4: Run tests to verify integration**

Run: `cargo test --lib --package alephcore agent_loop::message_builder`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/agent_loop/message_builder/builder.rs core/src/agent_loop/builder.rs
git commit -m "feat(agent_loop): integrate ContextProvider into MessageBuilder

Add context injection to message building:
- MessageBuilder holds Vec<Box<dyn ContextProvider>>
- Inject contexts sorted by priority (descending)
- Builder auto-adds SwarmContextProvider when Swarm enabled

Context injection happens at message build time, before LLM call.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

### Task 8: Add Event Statistics and Logging

**Files:**
- Modify: `core/src/agents/swarm/bus.rs` (add statistics)
- Modify: `core/src/agents/swarm/coordinator.rs` (add logging)

**Step 1: Add statistics tracking to AgentMessageBus**

```rust
// core/src/agents/swarm/bus.rs

use std::sync::atomic::{AtomicU64, Ordering};

pub struct AgentMessageBus {
    // ... existing fields ...
    stats: BusStatistics,
}

#[derive(Default)]
struct BusStatistics {
    total_published: AtomicU64,
    critical_count: AtomicU64,
    important_count: AtomicU64,
    info_count: AtomicU64,
}

impl AgentMessageBus {
    pub async fn publish(&self, event: AgentEvent) -> Result<()> {
        // Update statistics
        self.stats.total_published.fetch_add(1, Ordering::Relaxed);
        match &event {
            AgentEvent::Critical(_) => {
                self.stats.critical_count.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::Important(_) => {
                self.stats.important_count.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::Info(_) => {
                self.stats.info_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        // ... existing publish logic ...
    }

    /// Get event statistics
    pub fn statistics(&self) -> EventStatistics {
        EventStatistics {
            total_published: self.stats.total_published.load(Ordering::Relaxed),
            critical_count: self.stats.critical_count.load(Ordering::Relaxed),
            important_count: self.stats.important_count.load(Ordering::Relaxed),
            info_count: self.stats.info_count.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventStatistics {
    pub total_published: u64,
    pub critical_count: u64,
    pub important_count: u64,
    pub info_count: u64,
}
```

**Step 2: Add periodic logging in SwarmCoordinator**

```rust
// core/src/agents/swarm/coordinator.rs

impl SwarmCoordinator {
    /// Start background statistics logging
    pub fn start_statistics_logging(&self) {
        let bus = self.bus.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                std::time::Duration::from_secs(60)
            );

            loop {
                interval.tick().await;

                let stats = bus.statistics();
                tracing::info!(
                    total = stats.total_published,
                    critical = stats.critical_count,
                    important = stats.important_count,
                    info = stats.info_count,
                    "Swarm event statistics"
                );
            }
        });
    }
}
```

**Step 3: Start logging in Builder**

```rust
// core/src/agent_loop/builder.rs

impl<T, E, C> AgentLoopBuilder<T, E, C> {
    pub fn build(self) -> AgentLoop<T, E, C> {
        // ... existing build logic ...

        // If Swarm enabled, start statistics logging
        if let Some(ref coordinator) = self.swarm_coordinator {
            coordinator.start_statistics_logging();
        }

        // ... rest of build ...
    }
}
```

**Step 4: Run tests**

Run: `cargo test --lib --package alephcore swarm::bus::tests`

Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/agents/swarm/bus.rs core/src/agents/swarm/coordinator.rs core/src/agent_loop/builder.rs
git commit -m "feat(swarm): add event statistics and logging

Add event tracking and periodic logging:
- AgentMessageBus tracks event counts by tier
- SwarmCoordinator logs statistics every 60 seconds
- Builder auto-starts logging when Swarm enabled

Enables monitoring of event frequency and data quality in Phase 1.

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Phase 1 Validation

After completing all Phase 1 tasks, run comprehensive validation:

**Step 1: Run all tests**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/feature/swarm-agent-loop-integration
cargo test --lib --package alephcore
```

Expected: All tests PASS, no regressions

**Step 2: Check event statistics in logs**

```bash
# Run a test agent loop and check logs
RUST_LOG=info cargo run --bin aleph-server
# Look for "Swarm event statistics" log entries
```

Expected: Event counts visible in logs

**Step 3: Performance benchmark**

```bash
# Compare performance with/without Swarm
cargo bench --bench agent_loop_benchmark
```

Expected: <5% latency increase

**Step 4: Commit Phase 1 completion**

```bash
git commit --allow-empty -m "milestone: complete Phase 1 - Shadow Monitor Mode

Phase 1 deliverables:
- AgentLoopBuilder with optional SwarmCoordinator
- AgentLoopEvent semantic event types
- Event publishing at key AgentLoop points (shadow mode)
- ContextProvider trait and SwarmContextProvider
- MessageBuilder integration with context injection
- Event statistics and logging

Validation:
- All tests passing
- Event statistics visible in logs
- No performance regression (<5% latency)

Ready for Phase 2: Decision Broadcast

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

---

## Next Steps

Phase 1 (Shadow Monitor Mode) is now complete. The implementation plan continues with:

- **Phase 2: Decision Broadcast** - Enable context injection, register swarm_get_activity tool, end-to-end testing
- **Phase 3: Full Integration** - Expand event types, implement Tier 1 interruption, performance optimization

**Execution Options:**

1. **Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

2. **Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach would you like to use?
