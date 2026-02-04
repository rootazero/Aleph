# Design: Harden Sub-Agent Synchronization

## Overview

This document describes the architectural design for enhancing Aleph's sub-agent execution system to match OpenCode's efficiency and reliability.

## Current Architecture Analysis

### OpenCode's Approach (Reference)

OpenCode uses a **session-based synchronous execution model**:

```
1. Parent creates child session (Session.create with parentID)
2. Subscribes to Bus events for child session updates
3. Calls SessionPrompt.prompt() which BLOCKS until child completes
4. Collects all messages from child session
5. Aggregates tool execution summaries
6. Returns unified result to parent
```

**Key insight**: OpenCode treats each sub-agent as a **mini-conversation session** with full message history, not just a single function call.

### Aleph's Current Approach

Aleph uses an **event-driven fire-and-forget model**:

```
1. Parent calls dispatcher.dispatch(request)
2. SubAgentHandler receives SubAgentStarted event
3. SubAgent executes independently
4. SubAgentCompleted event emitted
5. SubAgentHandler removes session from tracking
⚠️ Parent never sees the result unless it polls!
```

**Gap**: No synchronous bridge between event emission and parent consumption.

---

## Proposed Architecture

### Component Design

#### 1. ExecutionCoordinator (New)

**Location**: `core/src/agents/sub_agents/coordinator.rs`

**Purpose**: Manages the lifecycle of sub-agent executions with synchronous wait capability.

```rust
pub struct ExecutionCoordinator {
    /// Pending executions awaiting results
    pending: RwLock<HashMap<String, PendingExecution>>,
    /// Completed results with TTL
    completed: RwLock<HashMap<String, CompletedExecution>>,
    /// Event bus subscription
    event_subscriber: Option<SubscriptionId>,
    /// Cleanup configuration
    config: CoordinatorConfig,
}

struct PendingExecution {
    request_id: String,
    created_at: Instant,
    /// Oneshot channel to signal completion
    completion_tx: Option<oneshot::Sender<SubAgentResult>>,
    /// Progress tracking
    tool_calls: Vec<ToolCallProgress>,
}

struct CompletedExecution {
    request_id: String,
    result: SubAgentResult,
    completed_at: Instant,
    /// Aggregated tool call summary
    tool_summary: Vec<ToolCallSummary>,
}

impl ExecutionCoordinator {
    /// Start a new execution and get a handle for waiting
    pub async fn start_execution(&self, request: &SubAgentRequest) -> ExecutionHandle;

    /// Wait for a specific execution to complete (with timeout)
    pub async fn wait_for_result(&self, request_id: &str, timeout: Duration)
        -> Result<SubAgentResult, ExecutionError>;

    /// Wait for multiple executions (for parallel dispatch)
    pub async fn wait_for_all(&self, request_ids: &[String], timeout: Duration)
        -> Vec<(String, Result<SubAgentResult, ExecutionError>)>;

    /// Called by event handler when execution completes
    pub async fn on_execution_completed(&self, result: SubAgentResult);

    /// Called for each tool call progress
    pub async fn on_tool_progress(&self, request_id: &str, progress: ToolCallProgress);
}
```

#### 2. ResultCollector (New)

**Location**: `core/src/agents/sub_agents/result_collector.rs`

**Purpose**: Aggregates all tool executions and artifacts from sub-agent runs.

```rust
pub struct ResultCollector {
    /// Tool call records indexed by request_id
    tool_records: RwLock<HashMap<String, Vec<ToolCallRecord>>>,
    /// Artifacts indexed by request_id
    artifacts: RwLock<HashMap<String, Vec<Artifact>>>,
}

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub id: String,              // Unique tool call ID
    pub tool_name: String,
    pub arguments: Value,
    pub status: ToolCallStatus,
    pub title: Option<String>,   // Completion title for UI
    pub started_at: Instant,
    pub completed_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed { output_preview: String },
    Failed { error: String },
}

impl ResultCollector {
    /// Record a tool call start
    pub async fn record_tool_start(&self, request_id: &str, call: ToolCallRecord);

    /// Update tool call status
    pub async fn update_tool_status(&self, request_id: &str, call_id: &str, status: ToolCallStatus);

    /// Record an artifact
    pub async fn record_artifact(&self, request_id: &str, artifact: Artifact);

    /// Get summary for a request (OpenCode-style)
    pub async fn get_summary(&self, request_id: &str) -> Vec<ToolCallSummary>;

    /// Get all artifacts for a request
    pub async fn get_artifacts(&self, request_id: &str) -> Vec<Artifact>;

    /// Clean up completed request data
    pub async fn cleanup(&self, request_id: &str);
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolCallState,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallState {
    pub status: String,  // "pending" | "running" | "completed" | "error"
    pub title: Option<String>,
}
```

#### 3. Enhanced SubAgentHandler

**Location**: `core/src/components/subagent_handler.rs` (existing, enhanced)

**Changes**:
- Inject `ExecutionCoordinator` and `ResultCollector`
- Forward events to coordinator
- Subscribe to tool call events

```rust
pub struct SubAgentHandler {
    registry: Arc<AgentRegistry>,
    active_sessions: RwLock<HashMap<String, SubAgentSession>>,
    // NEW
    coordinator: Arc<ExecutionCoordinator>,
    result_collector: Arc<ResultCollector>,
}

impl EventHandler for SubAgentHandler {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::SubAgentStarted,
            EventType::SubAgentCompleted,
            // NEW subscriptions
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
        ]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        match event {
            AlephEvent::SubAgentStarted(request) => {
                // Existing session tracking...
                // NEW: Initialize result collector for this request
                self.result_collector.init_request(&request.id).await;
            }
            AlephEvent::SubAgentCompleted(result) => {
                // Existing session cleanup...
                // NEW: Notify coordinator with aggregated summary
                let summary = self.result_collector.get_summary(&result.request_id).await;
                let enhanced_result = result.clone().with_tools_called(summary);
                self.coordinator.on_execution_completed(enhanced_result).await;
            }
            // NEW: Tool call tracking
            AlephEvent::ToolCallStarted(event) => {
                if let Some(request_id) = self.get_request_for_session(&event.session_id).await {
                    let record = ToolCallRecord::from_started(event);
                    self.result_collector.record_tool_start(&request_id, record).await;
                }
            }
            AlephEvent::ToolCallCompleted(event) => {
                if let Some(request_id) = self.get_request_for_session(&event.session_id).await {
                    self.result_collector.update_tool_status(
                        &request_id,
                        &event.call_id,
                        ToolCallStatus::Completed { output_preview: event.output_preview() }
                    ).await;
                }
            }
            _ => {}
        }
        Ok(vec![])
    }
}
```

#### 4. Enhanced SubAgentDispatcher

**Location**: `core/src/agents/sub_agents/dispatcher.rs` (existing, enhanced)

**Changes**:
- Integrate with ExecutionCoordinator
- Add synchronous dispatch methods
- Maintain request-result correlation

```rust
impl SubAgentDispatcher {
    // Existing methods...

    /// NEW: Dispatch and wait for result (synchronous from caller's perspective)
    pub async fn dispatch_sync(
        &self,
        request: SubAgentRequest,
        timeout: Duration,
    ) -> Result<SubAgentResult> {
        // 1. Start tracking with coordinator
        let handle = self.coordinator.start_execution(&request).await;

        // 2. Dispatch to sub-agent (existing logic)
        let agent = self.select_agent(&request)?;

        // 3. Execute (fire async)
        let request_id = request.id.clone();
        let agent_clone = agent.clone();
        tokio::spawn(async move {
            let result = agent_clone.execute(request).await;
            // Result will be emitted as SubAgentCompleted event
        });

        // 4. Wait for completion via coordinator
        self.coordinator.wait_for_result(&request_id, timeout).await
    }

    /// NEW: Dispatch multiple in parallel and wait for all
    pub async fn dispatch_parallel_sync(
        &self,
        requests: Vec<(SubAgentRequest, Option<String>)>,
        timeout: Duration,
    ) -> Vec<(String, Result<SubAgentResult>)> {
        // 1. Collect request IDs
        let request_ids: Vec<_> = requests.iter().map(|(r, _)| r.id.clone()).collect();

        // 2. Start all executions
        for (request, agent_id) in &requests {
            self.coordinator.start_execution(request).await;
        }

        // 3. Dispatch all (existing parallel logic)
        let futures: Vec<_> = requests.into_iter().map(|(request, agent_id)| {
            // ... spawn execution ...
        }).collect();

        // Don't await futures - let them complete asynchronously
        futures::future::join_all(futures);

        // 4. Wait for all results with correlation
        self.coordinator.wait_for_all(&request_ids, timeout).await
    }
}
```

---

## Event Flow Diagram

```
┌─────────────┐
│ Parent Agent│
└──────┬──────┘
       │
       │ dispatch_sync(request)
       ▼
┌──────────────────────┐
│ ExecutionCoordinator │
│ start_execution()    │
└──────────┬───────────┘
           │
           │ Creates PendingExecution with oneshot channel
           │
           ▼
┌──────────────────────┐    ┌─────────────────┐
│ SubAgentDispatcher   │───▶│ SubAgent        │
└──────────────────────┘    │ (MCP/Skill)     │
                            └────────┬────────┘
                                     │
                                     │ Tool Execution Loop
                                     ▼
                            ┌─────────────────┐
                            │ Event Bus       │
                            │ ToolCallStarted │
                            │ ToolCallResult  │
                            └────────┬────────┘
                                     │
         ┌───────────────────────────┼───────────────────┐
         ▼                           ▼                    ▼
┌────────────────┐        ┌────────────────┐    ┌────────────────┐
│ SubAgentHandler│        │ ResultCollector│    │ UI/Callback    │
│ (session track)│        │ (tool records) │    │ (progress)     │
└────────────────┘        └───────┬────────┘    └────────────────┘
                                  │
                                  │ On SubAgentCompleted
                                  ▼
                        ┌────────────────────┐
                        │ ExecutionCoordinator│
                        │ on_completed()      │
                        │ - Aggregate summary │
                        │ - Signal via oneshot│
                        └─────────┬──────────┘
                                  │
                                  │ Parent receives result
                                  ▼
                        ┌─────────────────────┐
                        │ SubAgentResult      │
                        │ + tools_called      │
                        │ + artifacts         │
                        └─────────────────────┘
```

---

## Context Propagation Enhancement

### Current Issue

`ExecutionContextInfo` is defined but rarely populated. OpenCode passes full session context including:
- Parent session ID
- Permission rules
- Model preferences
- Previous tool outputs

### Proposed Enhancement

```rust
impl SubAgentRequest {
    /// Create request with full context from parent execution
    pub fn from_parent_context(
        prompt: impl Into<String>,
        parent_ctx: &ExecutionContext,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            prompt: prompt.into(),
            target: None,
            context: HashMap::new(),
            max_iterations: parent_ctx.config.max_subagent_iterations,
            parent_session_id: Some(parent_ctx.session_id.clone()),
            execution_context: Some(ExecutionContextInfo {
                working_directory: parent_ctx.working_directory.clone(),
                current_app: parent_ctx.current_app.clone(),
                window_title: parent_ctx.window_title.clone(),
                original_request: Some(parent_ctx.original_prompt.clone()),
                history_summary: Some(parent_ctx.build_history_summary()),
                recent_steps: parent_ctx.recent_steps.clone(),
                metadata: parent_ctx.metadata.clone(),
            }),
        }
    }
}
```

---

## Configuration

### New Config Section

```toml
[subagent]
# Maximum time to wait for a sub-agent to complete
execution_timeout_ms = 300000  # 5 minutes

# How long to keep completed results before cleanup
result_ttl_ms = 3600000  # 1 hour

# Maximum concurrent sub-agent executions
max_concurrent = 5

# Enable real-time progress events
progress_events_enabled = true
```

---

## Error Handling

### Timeout Handling

```rust
pub enum ExecutionError {
    /// Sub-agent did not complete within timeout
    Timeout {
        request_id: String,
        elapsed: Duration,
        partial_summary: Option<Vec<ToolCallSummary>>,
    },
    /// Sub-agent execution failed
    ExecutionFailed {
        request_id: String,
        error: String,
        tools_completed: Vec<ToolCallSummary>,
    },
    /// No result found (cleaned up or never started)
    NotFound { request_id: String },
    /// Internal coordination error
    Internal(String),
}
```

### Graceful Degradation

1. **Timeout**: Return partial results if available
2. **Sub-agent failure**: Collect completed tools before failure
3. **Event loss**: Fallback to polling if events not received

---

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_coordinator_wait_for_single() {
    let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());
    let request = SubAgentRequest::new("test task");

    // Start execution
    coordinator.start_execution(&request).await;

    // Simulate completion in background
    let result = SubAgentResult::success(&request.id, "Done");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        coordinator.on_execution_completed(result).await;
    });

    // Wait should succeed
    let result = coordinator.wait_for_result(&request.id, Duration::from_secs(1)).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_parallel_result_ordering() {
    let dispatcher = setup_test_dispatcher().await;

    let requests = vec![
        SubAgentRequest::new("task 1"),
        SubAgentRequest::new("task 2"),
        SubAgentRequest::new("task 3"),
    ];
    let ids: Vec<_> = requests.iter().map(|r| r.id.clone()).collect();

    let results = dispatcher.dispatch_parallel_sync(
        requests.into_iter().map(|r| (r, None)).collect(),
        Duration::from_secs(10),
    ).await;

    // Results should be correlated with request IDs
    for (request_id, result) in results {
        assert!(ids.contains(&request_id));
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_mcp_subagent_with_tool_summary() {
    // End-to-end test with real MCP sub-agent
    // Verify tool calls are collected
    // Verify artifacts are aggregated
}
```

---

## Migration Path

### Phase 1: Core Infrastructure
- Add `ExecutionCoordinator`
- Add `ResultCollector`
- Minimal changes to existing code

### Phase 2: Integration
- Enhance `SubAgentHandler` with new event subscriptions
- Add `dispatch_sync` methods to dispatcher
- Update event types if needed

### Phase 3: Adoption
- Update callers to use sync dispatch
- Add configuration options
- Performance testing

---

## Alternatives Considered

### 1. Channel-per-Request Pattern
**Rejected**: Creates too many channels, harder to manage lifecycle.

### 2. Polling Pattern
**Rejected**: Inefficient, doesn't match event-driven architecture.

### 3. Callback Pattern (Current)
**Rejected**: Doesn't provide synchronous wait capability needed for proper task orchestration.

### 4. Session-Based Pattern (OpenCode Style)
**Partially Adopted**: We use request-based tracking rather than full session abstraction, but achieve similar result aggregation.

---

## References

- OpenCode `task.ts`: Sub-agent task execution
- OpenCode `prompt.ts`: Session prompt synchronous loop
- OpenCode `message-v2.ts`: Part state machine
- Aleph `agent_loop/mod.rs`: Current loop implementation
- Aleph `agents/sub_agents/dispatcher.rs`: Current dispatcher
