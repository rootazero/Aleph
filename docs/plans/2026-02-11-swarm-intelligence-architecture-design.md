# Swarm Intelligence Architecture Design

**Date**: 2026-02-11
**Status**: Design Phase
**Author**: Architecture Team

## Executive Summary

This design evolves Aleph's multi-agent system from "hierarchical delegation" to "swarm intelligence," enabling agents to transition from "command-driven" to "state-driven" collaboration. The core innovation is a layered event bus that provides horizontal communication, semantic aggregation, and collective memory.

## Motivation

### Current Limitations

Aleph's current multi-agent architecture (SubAgentDispatcher + DAG scheduling) suffers from:

1. **Vertical Isolation**: Sub-agents only communicate with parent agents, creating information silos
2. **Blind Execution**: Agents are unaware of parallel work, leading to duplicate efforts
3. **No Shared Context**: Discoveries made by Agent A are invisible to Agent B until task completion
4. **Reactive Coordination**: Parent agent must manually orchestrate all interactions

### Inspiration: Claude Code Agent Teams

Claude Code's `/team` mode demonstrates the power of:
- **Mailbox Broadcasting**: Agents broadcast status ("I'm working on X")
- **Shared Task List**: Global visibility with claim-based task assignment
- **Teammate Awareness**: Independent context with message bus synchronization

### Design Goals

1. **Zero-Blocking Latency**: Event publishing must not wait for LLM (microsecond response)
2. **Token Efficiency**: Aggregate N low-level events into 1 high-level insight
3. **Self-Hosted Friendly**: High-frequency logic in Rust, LLM only for async summarization
4. **Progressive Evolution**: Coexist with existing DAG scheduler, no breaking changes

## Architecture Overview

### Core Components

1. **AgentMessageBus** - Layered event bus (Tier 1/2/3)
2. **SemanticAggregator** - Hybrid aggregation (Rust rules + LLM summarization)
3. **ContextInjector** - Layered context delivery (interrupt/inject/query)
4. **CollectiveMemory** - Event archival + vector retrieval

### Design Principles

- **Fast Reflex + Slow Thinking**: Dual-loop control mimicking biological swarms
- **Information Density Over Volume**: Semantic aggregation prevents context overload
- **Tiered Awareness**: Critical events interrupt, important events inject, info events query
- **Collective Intelligence**: Shared discoveries amplify individual agent capabilities

## Component Design

### 1. AgentMessageBus

The neural backbone of swarm architecture, responsible for event publishing, subscription, and routing.

#### Event Hierarchy

```rust
// core/src/agents/swarm/events.rs
pub enum AgentEvent {
    // Tier 1: Critical (interrupt-driven)
    Critical(CriticalEvent),
    // Tier 2: Important (passive injection)
    Important(ImportantEvent),
    // Tier 3: Info (on-demand query)
    Info(InfoEvent),
}

pub enum CriticalEvent {
    BugRootCauseFound { location: String, description: String },
    TaskCancelled { task_id: String, reason: String },
    GlobalFailure { error: String },
}

pub enum ImportantEvent {
    // Semantically aggregated high-level events
    Hotspot { area: String, agent_count: usize, activity: String },
    ConfirmedInsight { symbol: String, confidence: f32 },
    SwarmStateSummary { summary: String, timestamp: u64 },
}

pub enum InfoEvent {
    ToolExecuted { agent_id: String, tool: String, path: Option<String> },
    FileAccessed { agent_id: String, path: String },
}
```

#### Publishing Interface

```rust
impl AgentMessageBus {
    pub async fn publish(&self, event: AgentEvent) -> Result<()>;
    pub async fn subscribe(&self, tier: EventTier) -> Receiver<AgentEvent>;
}
```

#### Event Tier Characteristics

| Tier | Frequency | Latency | Delivery | Use Case |
|------|-----------|---------|----------|----------|
| **Tier 1** | Rare | <1ms | Interrupt | Bug found, task cancelled |
| **Tier 2** | Moderate | <10ms | Next Think | Hotspot detected, swarm summary |
| **Tier 3** | High | N/A | On-demand | Tool calls, file reads |

### 2. SemanticAggregator

Implements "fast reflex + slow thinking" dual-loop control, transforming low-level events into high-level situational awareness.

#### Architecture

```rust
// core/src/agents/swarm/aggregator.rs
pub struct SemanticAggregator {
    // Fast path: rule engine (microsecond-level)
    rule_engine: RuleEngine,
    // Intelligence path: async summarizer (second-level)
    intelligence_layer: IntelligenceLayer,
    // Sliding window: cache recent events
    event_window: Arc<RwLock<SlidingWindow>>,
}
```

#### Fast Path: Rule Engine

Pattern-matching based aggregation for 90% of high-frequency events.

```rust
pub struct RuleEngine {
    rules: Vec<AggregationRule>,
}

pub struct AggregationRule {
    pattern: EventPattern,
    window_ms: u64,
    threshold: usize,
    output: fn(Vec<InfoEvent>) -> ImportantEvent,
}

// Example rule
impl RuleEngine {
    fn default_rules() -> Vec<AggregationRule> {
        vec![
            // Multiple agents accessing same path -> Hotspot
            AggregationRule {
                pattern: EventPattern::FileAccess { path_prefix: Some("auth/") },
                window_ms: 1000,
                threshold: 3,
                output: |events| ImportantEvent::Hotspot {
                    area: "auth/".into(),
                    agent_count: events.len(),
                    activity: "file_analysis".into(),
                },
            },
        ]
    }
}
```

#### Intelligence Path: Async Summarizer

LLM-powered summarization running every 5 seconds in background.

```rust
pub struct IntelligenceLayer {
    llm_client: Arc<dyn LLMProvider>,
    summary_interval: Duration,
}

impl IntelligenceLayer {
    pub async fn run(&self, bus: Arc<AgentMessageBus>) {
        let mut interval = tokio::time::interval(self.summary_interval);
        loop {
            interval.tick().await;
            let events = self.collect_recent_events().await;
            if let Ok(summary) = self.summarize_swarm_behavior(events).await {
                bus.publish(AgentEvent::Important(
                    ImportantEvent::SwarmStateSummary {
                        summary,
                        timestamp: now()
                    }
                )).await;
            }
        }
    }
}
```

#### Aggregation Strategy

| Event Type | Fast Path | Intelligence Path |
|------------|-----------|-------------------|
| File access patterns | ✅ Rule-based | ❌ |
| Symbol search clusters | ✅ Rule-based | ❌ |
| Complex behavior patterns | ❌ | ✅ LLM summarization |
| Swarm state summary | ❌ | ✅ Every 5 seconds |

### 3. ContextInjector

Implements layered context delivery strategy, using different injection methods based on event priority.

#### Architecture

```rust
// core/src/agents/swarm/context_injector.rs
pub struct ContextInjector {
    bus: Arc<AgentMessageBus>,
    // Sliding context viewport: keep only recent N updates
    context_window: Arc<RwLock<ContextWindow>>,
}

pub struct ContextWindow {
    max_entries: usize,  // Default: 5
    entries: VecDeque<SwarmContextEntry>,
}

pub struct SwarmContextEntry {
    timestamp: u64,
    event: ImportantEvent,
    summary: String,
}
```

#### Tier 1: Interrupt-Driven

For critical events that require immediate attention.

```rust
impl ContextInjector {
    pub async fn handle_critical_event(&self, event: CriticalEvent, agent_id: &str) {
        // 1. Abort current LLM generation (if streaming)
        self.abort_current_generation(agent_id).await;

        // 2. Inject event as System Feedback
        let feedback = format!(
            "[CRITICAL INTERRUPT] {}",
            self.format_critical_event(&event)
        );

        // 3. Trigger agent to re-enter Think phase
        self.trigger_rethink(agent_id, feedback).await;
    }
}
```

#### Tier 2: Passive Injection

Inject swarm state before Think phase in OTAF loop.

```rust
impl ContextInjector {
    pub async fn inject_swarm_state(&self, agent_id: &str) -> String {
        let window = self.context_window.read().await;

        // Return only recent 5 updates
        let recent_updates: Vec<String> = window.entries
            .iter()
            .take(5)
            .map(|entry| format!("[{}] {}",
                format_timestamp(entry.timestamp),
                entry.summary
            ))
            .collect();

        if recent_updates.is_empty() {
            return String::new();
        }

        format!(
            "\n## Swarm State (Team Awareness)\n{}\n",
            recent_updates.join("\n")
        )
    }
}
```

#### Tier 3: On-Demand Query

Agents actively query detailed event history when needed.

```rust
impl ContextInjector {
    pub async fn get_team_activity(&self, query: Option<String>) -> Vec<InfoEvent> {
        self.query_event_history(query).await
    }
}
```

#### Integration with Agent Loop

```rust
// In core/src/agent_loop/mod.rs
impl AgentLoop {
    async fn think_phase(&mut self) -> Result<ThinkResult> {
        // 1. Inject Swarm State (Tier 2)
        let swarm_context = self.context_injector
            .inject_swarm_state(&self.agent_id)
            .await;

        // 2. Add to System Prompt
        let enhanced_prompt = format!(
            "{}\n{}",
            swarm_context,
            self.base_prompt
        );

        // 3. Execute Think normally
        self.thinker.think(enhanced_prompt).await
    }
}
```

#### Delivery Strategy Matrix

| Tier | Method | Timing | Impact |
|------|--------|--------|--------|
| **Tier 1** | Interrupt | Immediate | Abort current work, rethink |
| **Tier 2** | Inject | Before Think | Enhance context, no interruption |
| **Tier 3** | Query | On-demand | Agent decides when to fetch |

### 4. CollectiveMemory

Persists event bus history, providing vector retrieval and structured query capabilities.

#### Architecture

```rust
// core/src/agents/swarm/collective_memory.rs
pub struct CollectiveMemory {
    // Subscribe to all Tier 3 events from bus
    event_subscriber: Receiver<AgentEvent>,
    // Vector store (reuse existing memory module)
    vector_store: Arc<VectorStore>,
    // Structured storage (SQLite)
    event_db: Arc<EventDatabase>,
}

pub struct EventDatabase {
    conn: Arc<RwLock<rusqlite::Connection>>,
}
```

#### Event Storage

```rust
impl EventDatabase {
    pub async fn store_event(&self, event: &InfoEvent) -> Result<()> {
        let conn = self.conn.write().await;
        conn.execute(
            "INSERT INTO swarm_events (timestamp, agent_id, event_type, data) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                now(),
                event.agent_id(),
                event.event_type(),
                serde_json::to_string(event)?
            ],
        )?;
        Ok(())
    }

    pub async fn query_events(&self, filter: EventFilter) -> Result<Vec<InfoEvent>> {
        // Support filtering by time, agent, path, etc.
        // ...
    }
}
```

#### Background Archival

```rust
impl CollectiveMemory {
    pub async fn run(&self) {
        while let Some(event) = self.event_subscriber.recv().await {
            match event {
                AgentEvent::Info(info_event) => {
                    // 1. Store to structured database
                    self.event_db.store_event(&info_event).await.ok();

                    // 2. If important discovery, add to vector store
                    if let Some(insight) = self.extract_insight(&info_event) {
                        self.vector_store.add_fact(insight).await.ok();
                    }
                }
                _ => {}
            }
        }
    }
}
```

#### Hybrid Search

```rust
impl CollectiveMemory {
    pub async fn search_team_history(&self, query: &str) -> Result<Vec<String>> {
        // Hybrid retrieval: vector search + structured query
        let vector_results = self.vector_store.search(query, 5).await?;
        let recent_events = self.event_db.query_events(
            EventFilter::recent(Duration::from_secs(3600))
        ).await?;

        // Merge results
        Ok(self.merge_results(vector_results, recent_events))
    }
}
```

#### Exposed as AlephTool

```rust
// core/src/builtin_tools/swarm_tools.rs
pub struct GetTeamActivityTool {
    collective_memory: Arc<CollectiveMemory>,
}

#[async_trait]
impl AlephTool for GetTeamActivityTool {
    fn name(&self) -> &str { "get_team_activity" }

    fn description(&self) -> &str {
        "Query what other agents in the team have been working on. \
         Use this when you need context about parallel work or want to \
         avoid duplicate efforts."
    }

    async fn execute(&self, args: ToolArgs) -> Result<ToolResult> {
        let query = args.get_string("query")?;
        let results = self.collective_memory.search_team_history(&query).await?;
        Ok(ToolResult::success(results.join("\n")))
    }
}
```

## Data Flow & Integration

### Complete Data Flow

```
User Request → Main Agent
    ↓
Main Agent decomposes task → SubAgentDispatcher
    ↓
Sub-Agent A executes tool → Publishes InfoEvent (Tier 3)
    ↓                           ↓
    ↓                       AgentMessageBus
    ↓                           ↓
    ↓                   SemanticAggregator (rule engine)
    ↓                           ↓
    ↓                   Detects Hotspot → Publishes ImportantEvent (Tier 2)
    ↓                           ↓
Sub-Agent B enters Think phase
    ↓
ContextInjector.inject_swarm_state()
    ↓
Injects recent 5 updates to Prompt
    ↓
Sub-Agent B aware of A's work, adjusts strategy
    ↓
Avoids duplicate work, collaborates to complete task

// Running in parallel
IntelligenceLayer (background)
    ↓
Collects events every 5 seconds → LLM summarization
    ↓
Publishes SwarmStateSummary (Tier 2)
    ↓
All agents perceive in next Think phase

// Persistence
CollectiveMemory (background)
    ↓
Subscribes to all Tier 3 events
    ↓
Stores to SQLite + vector store
    ↓
Provides get_team_activity tool for queries
```

### Integration with Existing Architecture

```rust
// core/src/agents/mod.rs
pub struct SwarmCoordinator {
    pub bus: Arc<AgentMessageBus>,
    pub aggregator: Arc<SemanticAggregator>,
    pub injector: Arc<ContextInjector>,
    pub memory: Arc<CollectiveMemory>,
}

impl SwarmCoordinator {
    pub async fn initialize(config: SwarmConfig) -> Result<Self> {
        let bus = Arc::new(AgentMessageBus::new());
        let aggregator = Arc::new(SemanticAggregator::new(bus.clone()));
        let injector = Arc::new(ContextInjector::new(bus.clone()));
        let memory = Arc::new(CollectiveMemory::new(bus.clone()));

        // Start background tasks
        tokio::spawn(aggregator.clone().run());
        tokio::spawn(memory.clone().run());

        Ok(Self { bus, aggregator, injector, memory })
    }
}

// Integration with existing SubAgentDispatcher
impl SubAgentDispatcher {
    pub fn with_swarm(
        tool_registry: Arc<RwLock<ToolRegistry>>,
        swarm: Arc<SwarmCoordinator>,
    ) -> Self {
        let mut dispatcher = Self::with_defaults(tool_registry);

        // Inject swarm capabilities to all sub-agents
        for agent in dispatcher.agents.values_mut() {
            agent.set_swarm_coordinator(swarm.clone());
        }

        dispatcher
    }
}
```

### Coexistence with DAG Scheduler

The swarm architecture **does not replace** the existing DAG scheduler. Instead:

- **DAG**: Handles task dependencies and execution order (vertical coordination)
- **Swarm**: Provides horizontal awareness and collaboration (lateral coordination)
- **Together**: DAG ensures correctness, Swarm enhances efficiency

## Implementation Roadmap

### Phase 1: Event Bus Foundation (1-2 weeks)

**Goal**: Establish horizontal communication capability

- [ ] Implement `AgentMessageBus` (Pub/Sub pattern)
- [ ] Define `AgentEvent` enum (Tier 1/2/3)
- [ ] Add event publishing interface to `SubAgent` trait
- [ ] Unit tests: event publish/subscribe mechanism

**Deliverables**:
- `core/src/agents/swarm/bus.rs`
- `core/src/agents/swarm/events.rs`
- Integration tests

### Phase 2: Semantic Aggregator (2-3 weeks)

**Goal**: Implement "fast reflex + slow thinking"

- [ ] Implement `RuleEngine` (pattern matching based)
- [ ] Define default aggregation rules (file access, symbol search, etc.)
- [ ] Implement `IntelligenceLayer` (async LLM summarization)
- [ ] Integration tests: verify event aggregation effectiveness

**Deliverables**:
- `core/src/agents/swarm/aggregator.rs`
- `core/src/agents/swarm/rules.rs`
- `core/src/agents/swarm/intelligence.rs`

### Phase 3: Context Injector (1-2 weeks)

**Goal**: Implement layered delivery strategy

- [ ] Implement `ContextInjector`
- [ ] Integrate into `AgentLoop` Think phase
- [ ] Implement Tier 1 interrupt mechanism
- [ ] Implement sliding context viewport (keep recent 5)
- [ ] Integration tests: verify context injection doesn't impact performance

**Deliverables**:
- `core/src/agents/swarm/context_injector.rs`
- Modified `core/src/agent_loop/mod.rs`

### Phase 4: Collective Memory (1-2 weeks)

**Goal**: Persist event history

- [ ] Implement `CollectiveMemory` (SQLite + vector store)
- [ ] Implement `GetTeamActivityTool`
- [ ] Integrate with existing Memory system
- [ ] Performance tests: verify query efficiency

**Deliverables**:
- `core/src/agents/swarm/collective_memory.rs`
- `core/src/builtin_tools/swarm_tools.rs`
- Database schema migration

### Phase 5: End-to-End Integration (1 week)

**Goal**: Complete swarm collaboration

- [ ] Create `SwarmCoordinator` for unified management
- [ ] Integrate with `SubAgentDispatcher`
- [ ] Write example scenario (multi-agent collaborative refactoring task)
- [ ] Performance benchmarks
- [ ] Documentation updates

**Deliverables**:
- `core/src/agents/swarm/coordinator.rs`
- Example scenarios in `examples/`
- Updated architecture documentation

### Total Effort Estimate

**6-10 weeks** (depending on team size and complexity)

### Success Metrics

- **Horizontal Awareness**: Agents can perceive other agents' work
- **Token Efficiency**: 30%+ reduction through semantic aggregation
- **Duplicate Work**: 50%+ reduction through hotspot detection
- **Task Completion Time**: 20%+ reduction through collaborative avoidance

## Technical Considerations

### Performance

- **Event Bus Latency**: Target <1ms for Tier 1, <10ms for Tier 2
- **Memory Overhead**: Sliding window limited to 5 entries per agent
- **LLM Cost**: Intelligence layer runs every 5 seconds, ~100 tokens per summary

### Scalability

- **Agent Count**: Designed for 2-10 concurrent agents
- **Event Rate**: Can handle 1000+ events/second with rule engine
- **Storage**: SQLite can handle millions of events with proper indexing

### Security

- **Event Isolation**: Agents can only see events from same session
- **Tool Access**: `get_team_activity` respects existing permission system
- **Data Privacy**: Event history stored locally, no external transmission

## Future Enhancements

### Phase 6: Peer-to-Peer Negotiation

Allow Sub-Agent A to directly request help from Sub-Agent B without parent mediation.

```rust
pub trait SubAgent {
    async fn request_assistance(&self, target: &str, request: AssistanceRequest) -> Result<()>;
}
```

### Phase 7: Conflict Resolution Agent

Dedicated agent monitoring the bus for contradictory modifications, initiating arbitration.

```rust
pub struct ConflictResolver {
    bus: Arc<AgentMessageBus>,
}

impl ConflictResolver {
    pub async fn detect_conflicts(&self) -> Vec<Conflict>;
    pub async fn arbitrate(&self, conflict: Conflict) -> Resolution;
}
```

### Phase 8: Dynamic Team Scaling

Automatically spawn ephemeral agents based on task backlog, destroy after completion.

```rust
impl SwarmCoordinator {
    pub async fn scale_up(&self, task_count: usize) -> Vec<AgentId>;
    pub async fn scale_down(&self, agents: Vec<AgentId>);
}
```

## Conclusion

This design transforms Aleph from a "command-driven" multi-agent system to a "state-driven" swarm intelligence platform. By introducing layered event bus, semantic aggregation, and collective memory, agents gain horizontal awareness and collaborative capabilities while maintaining the robustness of existing DAG scheduling.

The architecture prioritizes:
- **Zero-blocking latency** through async design
- **Token efficiency** through semantic aggregation
- **Self-hosted friendliness** through Rust-first implementation
- **Progressive evolution** through coexistence with existing systems

Implementation can proceed incrementally, with each phase delivering tangible value independently.

