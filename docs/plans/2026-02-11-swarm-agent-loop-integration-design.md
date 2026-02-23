# Swarm Intelligence Architecture - Agent Loop Integration Design

> **Date**: 2026-02-11
> **Status**: Design Complete, Ready for Implementation
> **Related**: [Swarm Architecture Design](2026-02-11-swarm-intelligence-architecture-design.md)

## Overview

This document describes the integration of Swarm Intelligence Architecture into Aleph's Agent Loop, enabling horizontal agent collaboration through event-driven communication, context injection, and collective memory.

## Architecture Overview

### Core Components Relationship

```
AgentLoopBuilder (Constructor)
    ├─> SwarmCoordinator (Optional)
    │   ├─> AgentMessageBus (Event Bus)
    │   ├─> SemanticAggregator (Semantic Aggregation)
    │   ├─> ContextInjector (Context Injection)
    │   └─> CollectiveMemory (Collective Memory)
    │
    ├─> MessageBuilder
    │   └─> ContextProvider[] (Abstract Interface)
    │       └─> SwarmContextProvider (Implementation)
    │
    └─> ToolRegistry
        └─> swarm_get_activity (Dynamic Registration)
```

### Data Flow

1. **Event Publishing** - AgentLoop publishes semantic events at key operation points
2. **Event Aggregation** - SwarmCoordinator subscribes to events, processes by tier (Tier 1/2/3)
3. **Context Injection** - MessageBuilder retrieves team status via ContextProvider
4. **Active Query** - Agent queries history via `swarm_get_activity` tool

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Construction Pattern** | Builder Pattern | Avoids parameter explosion, supports optional component composition |
| **Event Source** | Semantic naming + Coordinator classification | Decouples AgentLoop from tier logic, flexible priority adjustment |
| **Injection Mechanism** | ContextProvider abstraction + tiered delivery | Extensible, testable, supports multiple context sources |
| **Tool Discovery** | Builder auto-injection | Strong consistency, ensures tools available when Swarm enabled |

## Design Details

### 1. AgentLoopBuilder Pattern

**Problem**: AgentLoop constructor has too many optional parameters (EventBus, OverflowDetector, SwarmCoordinator, etc.), leading to parameter explosion.

**Solution**: Introduce Builder pattern for flexible component composition.

```rust
pub struct AgentLoopBuilder<T, E, C> {
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    config: LoopConfig,

    // Optional components
    event_bus: Option<Arc<EventBus>>,
    overflow_detector: Option<Arc<OverflowDetector>>,
    swarm_coordinator: Option<Arc<SwarmCoordinator>>,
    context_providers: Vec<Box<dyn ContextProvider>>,
}

impl<T, E, C> AgentLoopBuilder<T, E, C> {
    pub fn new(thinker: Arc<T>, executor: Arc<E>, compressor: Arc<C>) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config: LoopConfig::default(),
            event_bus: None,
            overflow_detector: None,
            swarm_coordinator: None,
            context_providers: Vec::new(),
        }
    }

    pub fn with_config(mut self, config: LoopConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    pub fn with_overflow_detector(mut self, detector: Arc<OverflowDetector>) -> Self {
        self.overflow_detector = Some(detector);
        self
    }

    pub fn with_swarm(mut self, coordinator: Arc<SwarmCoordinator>) -> Self {
        self.swarm_coordinator = Some(coordinator);
        self
    }

    pub fn build(self) -> AgentLoop<T, E, C> {
        // Auto-inject Swarm tools and ContextProvider
        if let Some(ref coordinator) = self.swarm_coordinator {
            // 1. Register collaboration tools
            let tools = coordinator.get_collaboration_tools();
            for tool in tools {
                if let Err(e) = self.executor.tool_registry().register(tool) {
                    tracing::warn!("Failed to register swarm tool: {}", e);
                }
            }

            // 2. Inject SwarmContextProvider
            let provider = SwarmContextProvider::new(
                coordinator.context_injector()
            );
            self.context_providers.push(Box::new(provider));

            // 3. Start Intelligence Layer (background aggregation task)
            let bus = coordinator.bus();
            let aggregator = coordinator.aggregator();
            let event_window = coordinator.event_window();
            tokio::spawn(async move {
                aggregator.run(bus, event_window).await;
            });
        }

        // Build MessageBuilder (inject all ContextProviders)
        let message_builder = MessageBuilder::new(self.config.clone())
            .with_providers(self.context_providers);

        // Build AgentLoop
        AgentLoop {
            thinker: self.thinker,
            executor: self.executor,
            compressor: self.compressor,
            config: self.config,
            message_builder,
            swarm_coordinator: self.swarm_coordinator,
            event_bus: self.event_bus,
            overflow_detector: self.overflow_detector,
        }
    }
}
```

**Key Features**:
- **Auto-injection**: build() automatically registers tools and ContextProvider
- **Strong consistency**: SwarmCoordinator presence guarantees collaboration tools
- **Backward compatible**: Keep existing constructors as shortcuts

### 2. Event Publishing Mechanism

**Problem**: AgentLoop needs to publish events at key points, but shouldn't know about Tier classification.

**Solution**: Semantic event naming + SwarmCoordinator classification.

#### Semantic Event Definition

```rust
/// AgentLoop publishes semantic events (not Tier-classified)
pub enum AgentLoopEvent {
    ActionInitiated {
        agent_id: String,
        action_type: String,
        target: Option<String>,  // File path, tool name, etc.
    },
    ActionCompleted {
        agent_id: String,
        action_type: String,
        result: ActionResult,
        duration_ms: u64,
    },
    DecisionMade {
        agent_id: String,
        decision: String,  // "refactor Auth module", "fix dependency conflict"
        affected_files: Vec<String>,
    },
    InsightCaptured {
        agent_id: String,
        insight: String,  // Error, contradiction, major discovery
        severity: InsightSeverity,
    },
}
```

#### Publishing Trigger Points (Phase 1)

| Trigger Point | Domain Meaning | Event Tier | Core Value |
|---------------|----------------|------------|------------|
| Tool Execution Start | ActionInitiated | Tier 3 (Info) | Prevent two agents from modifying same file (hotspot warning) |
| Tool Execution End | ActionCompleted | Tier 2 (Important) | Share key findings or modification results from tool output |
| Thinking Phase End | DecisionMade | Tier 2 (Important) | Broadcast upcoming plan, implement "task territory" soft isolation |
| Agent Feedback Loop | InsightCaptured | Tier 1 (Critical) | When tool errors or major logic contradictions found, immediately seek team assistance |

```rust
// In AgentLoop::run() at key locations

// 1. Tool execution start (Tier 3 - Info)
callback.on_action_start(&action).await;
if let Some(ref swarm) = self.swarm_coordinator {
    swarm.publish_event(AgentLoopEvent::ActionInitiated {
        agent_id: state.session_id.clone(),
        action_type: action.action_type(),
        target: action.target(),
    }).await;
}

// 2. Tool execution complete (Tier 2 - Important)
callback.on_action_done(&action, &result).await;
if let Some(ref swarm) = self.swarm_coordinator {
    swarm.publish_event(AgentLoopEvent::ActionCompleted {
        agent_id: state.session_id.clone(),
        action_type: action.action_type(),
        result: result.clone(),
        duration_ms,
    }).await;
}

// 3. Decision broadcast (Tier 2 - Important, Phase 1 priority)
if let Decision::UseTool { tool_name, arguments } = &thinking.decision {
    if let Some(ref swarm) = self.swarm_coordinator {
        swarm.publish_event(AgentLoopEvent::DecisionMade {
            agent_id: state.session_id.clone(),
            decision: format!("Using {} to {}", tool_name, extract_intent(arguments)),
            affected_files: extract_files(arguments),
        }).await;
    }
}
```

#### SwarmCoordinator Tiered Processing

```rust
impl SwarmCoordinator {
    pub async fn publish_event(&self, event: AgentLoopEvent) {
        // Convert to internal event and classify
        let swarm_event = match event {
            AgentLoopEvent::ActionCompleted { .. } => {
                AgentEvent::Important(ImportantEvent::ToolExecuted { .. })
            }
            AgentLoopEvent::DecisionMade { .. } => {
                AgentEvent::Important(ImportantEvent::DecisionBroadcast { .. })
            }
            AgentLoopEvent::InsightCaptured { severity: Critical, .. } => {
                AgentEvent::Critical(CriticalEvent::ErrorDetected { .. })
            }
            _ => AgentEvent::Info(InfoEvent::ActionStarted { .. }),
        };

        // Publish to bus
        self.bus.publish(swarm_event).await;
    }
}
```

**Key Features**:
- **Decoupling**: AgentLoop doesn't know Tier classification, only publishes semantic events
- **Flexibility**: SwarmCoordinator handles classification and routing, can dynamically adjust priority
- **Phase 1 Priority**: Implement DecisionMade (decision broadcast) first, highest ROI

### 3. ContextProvider Interface

**Problem**: MessageBuilder shouldn't directly hold ContextInjector, causing circular dependencies or excessive responsibility.

**Solution**: Introduce ContextProvider trait for extensible context sources.

#### Abstract Interface

```rust
/// Context provider abstract interface
pub trait ContextProvider: Send + Sync {
    /// Get context content
    fn get_context(&self) -> Option<String>;

    /// Priority (determines position in Prompt)
    /// Higher values appear first, Critical context should have highest priority
    fn priority(&self) -> i32;

    /// Provider name (for debugging and logging)
    fn name(&self) -> &str;
}
```

#### SwarmContextProvider Implementation

```rust
pub struct SwarmContextProvider {
    context_injector: Arc<ContextInjector>,
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
        100  // High priority, but lower than Critical interruption
    }

    fn name(&self) -> &str {
        "swarm_context"
    }
}

impl SwarmContextProvider {
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
                    if let Some(files) = extract_files(result) {
                        xml.push_str(&format!(
                            "    <files>{}</files>\n",
                            files.join(", ")
                        ));
                    }
                    xml.push_str("  </agent>\n");
                }
                ImportantEvent::DecisionBroadcast { agent_id, decision, affected_files } => {
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
}
```

#### MessageBuilder Integration

```rust
impl MessageBuilder {
    pub fn build(&self, state: &LoopState, tools: &[Tool]) -> Vec<Message> {
        let mut messages = vec![
            self.build_system_message(),
            // ... other messages
        ];

        // Inject all ContextProvider contexts (sorted by priority)
        let mut providers = self.context_providers.clone();
        providers.sort_by_key(|p| -p.priority());  // Descending order

        for provider in providers {
            if let Some(context) = provider.get_context() {
                messages.push(Message::system(context));
            }
        }

        messages
    }
}
```

**Key Features**:
- **Decoupling**: MessageBuilder doesn't know about Swarm, only ContextProvider interface
- **Extensibility**: Can easily add SecurityContextProvider, PerformanceContextProvider, etc.
- **Format Standardization**: Use XML tags to distinguish "team broadcast" from "own memory"
- **Priority Control**: priority() determines context position in Prompt

### 4. Tool Registration Mechanism

**Problem**: GetTeamActivityTool needs to be registered to tool server for agent query.

**Solution**: Dynamic registration via Builder auto-injection.

#### GetTeamActivityTool Implementation

```rust
#[derive(Clone)]
pub struct GetTeamActivityTool {
    memory: Arc<CollectiveMemory>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetTeamActivityArgs {
    /// Query string (e.g., "agent:BEE-01", "path:src/auth", "recent:10")
    query: String,
}

#[derive(Serialize, JsonSchema)]
pub struct GetTeamActivityOutput {
    events: Vec<AgentEvent>,
    count: usize,
}

#[async_trait]
impl AlephTool for GetTeamActivityTool {
    const NAME: &'static str = "swarm_get_activity";
    const DESCRIPTION: &'static str =
        "Query what other agents are doing or have done recently. \
         Use this to avoid duplicate work or coordinate with teammates. \
         Examples: 'agent:BEE-01' (specific agent), 'path:src/auth' (file-based), \
         'recent:10' (last 10 events)";

    type Args = GetTeamActivityArgs;
    type Output = GetTeamActivityOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let query = TeamHistoryQuery::from_string(&args.query)?;
        let events = self.memory.search_team_history(query).await?;
        Ok(GetTeamActivityOutput {
            count: events.len(),
            events,
        })
    }
}
```

#### SwarmCoordinator Tool Provision

```rust
impl SwarmCoordinator {
    /// Get collaboration tool list (for Builder auto-registration)
    pub fn get_collaboration_tools(&self) -> Vec<Box<dyn AlephTool>> {
        vec![
            Box::new(GetTeamActivityTool::new(
                self.collective_memory.clone()
            )),
            // Future collaboration tools:
            // Box::new(BroadcastMessageTool::new(...)),
            // Box::new(ClaimTaskTool::new(...)),
        ]
    }

    /// Get ContextInjector reference (for SwarmContextProvider)
    pub fn context_injector(&self) -> Arc<ContextInjector> {
        self.context_injector.clone()
    }
}
```

**Key Features**:
- **Auto-injection**: Builder::build() automatically registers tools, no manual calls needed
- **Strong consistency**: SwarmCoordinator presence guarantees swarm_get_activity tool
- **Dependency injection**: Pass Arc<CollectiveMemory> to tool instance
- **Naming convention**: Use `swarm_` prefix to clearly identify collaboration tools
- **Background task**: Automatically start Intelligence Layer aggregation task

## Implementation Phases

### Phase 1: Infrastructure (Shadow Monitor Mode)

**Goal**: Establish event collection and validation mechanism without interfering with existing Agent behavior.

**Tasks**:
1. Create AgentLoopBuilder
   - Implement with_swarm() method
   - Implement build() auto-injection logic
   - Keep existing constructors as shortcuts

2. Implement event publishing (shadow mode)
   - Publish AgentLoopEvent at AgentLoop key points
   - SwarmCoordinator silently collects, doesn't inject context
   - Record event frequency and data quality metrics

3. Implement ContextProvider interface
   - Define trait ContextProvider
   - Implement SwarmContextProvider (not enabled yet)
   - MessageBuilder integrate providers list

4. Validation criteria
   - EventBus receives events normally
   - Event classification correct (Tier 1/2/3)
   - No performance regression (<5% latency increase)
   - Event statistics visible in logs

### Phase 2: Decision Broadcast (Minimum Viable)

**Goal**: Implement complete DecisionMade event flow, validate collaboration value.

**Tasks**:
1. Enable DecisionMade event injection
   - SwarmContextProvider starts returning context
   - Only inject DecisionBroadcast type events
   - Use XML format specification

2. Register swarm_get_activity tool
   - Implement GetTeamActivityTool
   - Builder auto-registration
   - Test tool invocation

3. End-to-end testing
   - Start 2 Agent instances
   - Agent A broadcasts decision
   - Agent B receives and perceives A's plan
   - Verify B avoids A's work area

4. Validation criteria
   - Agent can see <team_awareness> context
   - swarm_get_activity tool available
   - Collaboration scenario tests pass
   - User feedback collected

### Phase 3: Full Integration (Production Ready)

**Goal**: Implement all event types and Tier 1 interruption mechanism.

**Tasks**:
1. Expand event types
   - ActionCompleted (Tool execution complete)
   - InsightCaptured (Error/contradiction discovery)
   - File access events (FileAccessed)

2. Implement Tier 1 interruption
   - Monitor Critical events in AgentLoop execution layer
   - Use CancellationToken to interrupt LLM inference
   - Force inject emergency context

3. Performance optimization
   - Event aggregation batching
   - Context caching
   - Sliding window size tuning

4. Validation criteria
   - All event types work normally
   - Tier 1 interruption response time <100ms
   - Multi-agent collaboration scenarios stable
   - Production environment stress test passed

## Key Milestones

| Phase | Deliverable | Validation Criteria | Estimated Time |
|-------|-------------|---------------------|----------------|
| Phase 1 | Shadow Monitor | Event collection normal, no performance regression | 2-3 days |
| Phase 2 | Decision Broadcast | 2-Agent collaboration test passed | 3-4 days |
| Phase 3 | Full Integration | Production stress test passed | 4-5 days |

## Risk Control

- **Feature Flag** - Control Swarm feature enablement via configuration switch
- **Progressive Deployment** - Validate in test environment first, then gradually roll out
- **Rollback Mechanism** - Keep existing constructors, can quickly revert
- **Monitoring Metrics** - Event frequency, latency, memory usage

## Team Communication Protocol

To prevent agents from confusing "own memory" with "team broadcast", use XML tags for injection:

```xml
<team_awareness timestamp="2026-02-11T05:34:00Z">
  <agent id="BEE-01" status="working">
    <action>Refactoring Auth module</action>
    <files>src/auth/mod.rs, src/auth/session.rs</files>
  </agent>
  <agent id="BEE-03" status="blocked">
    <issue>Cargo.toml dependency conflict detected</issue>
  </agent>
  <summary>Team is focusing on environment configuration issues</summary>
</team_awareness>
```

This allows LLM to clearly distinguish between "my memory" and "team broadcast".

## Summary

This integration design achieves:

✅ **Builder Pattern** - Flexible component composition, avoids parameter explosion
✅ **Decoupled Architecture** - AgentLoop unaware of Swarm, integrates via abstract interfaces
✅ **Tiered Events** - Semantic publishing + Coordinator logical classification
✅ **Standardized Protocol** - XML format team communication protocol
✅ **Progressive Implementation** - Shadow monitor → Decision broadcast → Full integration

The design is complete and ready for implementation.
