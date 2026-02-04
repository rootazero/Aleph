# Event-Driven Agentic Loop Architecture Design

> **Date**: 2026-01-18
> **Status**: Draft
> **Reference**: OpenCode project analysis

## Overview

This document describes the redesign of Aleph's agent execution system, transitioning from a single-pass processing model to an event-driven agentic loop architecture. The goal is to enable multi-step task execution with intelligent planning, tool chaining, and robust error recovery.

## Design Decisions

| Decision Point | Choice |
|----------------|--------|
| Overall Mode | AI Planning + Agentic Loop Combined |
| Loop Granularity | Tool-level Loop (result feedback after each tool call) |
| Planning Trigger | Smart Trigger (auto-detect complexity) |
| Agent Model | Introduce Sub-agents (explore, coder, researcher) |
| State Persistence | Full Execution State + SQLite |
| Protection Mechanisms | Full Suite (doom loop, retry, compaction, max steps) |

---

## Part 1: Event Bus Core Design

### EventBus Architecture

Using type-safe Channel pattern in Rust instead of dynamic pub-sub:

```rust
// Core event bus
pub struct EventBus {
    // Use tokio broadcast channel for multiple subscribers
    sender: broadcast::Sender<AlephEvent>,
    // Event history for state recovery
    history: Arc<RwLock<Vec<TimestampedEvent>>>,
}

// Unified event enum (type-safe)
pub enum AlephEvent {
    // Input events
    InputReceived(InputEvent),

    // Planning events
    PlanRequested(PlanRequest),
    PlanCreated(TaskPlan),

    // Execution events
    ToolCallRequested(ToolCallRequest),
    ToolCallStarted(ToolCallStarted),
    ToolCallCompleted(ToolCallResult),
    ToolCallFailed(ToolCallError),
    ToolCallRetrying(ToolCallRetry),

    // Loop control
    LoopContinue(LoopState),
    LoopStop(StopReason),

    // Session events
    SessionCreated(SessionInfo),
    SessionUpdated(SessionDiff),
    SessionResumed(ExecutionSession),
    SessionCompacted(CompactionInfo),

    // Sub-agent events
    SubAgentStarted(SubAgentRequest),
    SubAgentCompleted(SubAgentResult),

    // User interaction
    UserQuestionAsked(UserQuestion),
    UserResponseReceived(UserResponse),

    // AI response
    AiResponseGenerated(AiResponse),
}

pub struct TimestampedEvent {
    pub event: AlephEvent,
    pub timestamp: i64,
    pub sequence: u64,
}
```

**Key Design Points**:
- Use `tokio::sync::broadcast` instead of `std::sync::mpsc` for multiple subscribers
- Event history saved to memory, periodically flushed to SQLite
- Each event has timestamp for replay and debugging

---

## Part 2: Component Subscription Pattern

### Core Components and Event Subscriptions

Each component acts as an independent event handler, subscribing to specific events and publishing new ones:

```rust
// Component trait - all handlers implement this interface
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Event types this handler cares about
    fn subscriptions(&self) -> Vec<EventType>;

    /// Handle event, may produce new events
    async fn handle(&self, event: &AlephEvent, ctx: &EventContext)
        -> Result<Vec<AlephEvent>>;
}

// Event context - shared state
pub struct EventContext {
    pub session: Arc<RwLock<ExecutionSession>>,
    pub config: Arc<AlephConfig>,
    pub tools: Arc<ToolRegistry>,
    pub bus: EventBus,
    pub abort_signal: Arc<AtomicBool>,
}
```

### Component Registry

```rust
pub struct ComponentRegistry {
    handlers: Vec<Arc<dyn EventHandler>>,
}

impl ComponentRegistry {
    pub fn register(&mut self, handler: Arc<dyn EventHandler>) {
        self.handlers.push(handler);
    }

    // Start event listening for all components
    pub async fn start(&self, bus: &EventBus, ctx: Arc<EventContext>) {
        for handler in &self.handlers {
            let mut rx = bus.subscribe();
            let handler = Arc::clone(handler);
            let ctx = Arc::clone(&ctx);

            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    if handler.subscriptions().contains(&event.event_type()) {
                        if let Ok(new_events) = handler.handle(&event, &ctx).await {
                            for new_event in new_events {
                                let _ = ctx.bus.publish(new_event).await;
                            }
                        }
                    }
                }
            });
        }
    }
}
```

### Component List

| Component | Subscribes To | Publishes |
|-----------|---------------|-----------|
| IntentAnalyzer | InputReceived | PlanRequested / ToolCallRequested |
| TaskPlanner | PlanRequested | PlanCreated |
| ToolExecutor | ToolCallRequested | ToolCallStarted / ToolCallCompleted / ToolCallFailed |
| LoopController | ToolCallCompleted, ToolCallFailed, PlanCreated | LoopContinue / LoopStop / ToolCallRequested |
| SessionRecorder | All events | SessionUpdated |
| SessionCompactor | LoopContinue | SessionCompacted |
| CallbackBridge | UI-relevant events | (none, calls Swift callbacks) |

---

## Part 3: Agentic Loop Implementation

### LoopController - Loop Control Core

The "brain" of the system, deciding whether to continue or stop after each tool execution:

```rust
pub struct LoopController {
    config: LoopConfig,
    llm_client: Arc<dyn LlmProvider>,
}

pub struct LoopConfig {
    pub max_iterations: u32,          // Max loop count, default 50
    pub doom_loop_threshold: u32,     // Doom loop detection threshold, default 3
    pub max_tokens_per_session: u64,  // Token limit
    pub retry_policy: RetryPolicy,    // Retry strategy
}

#[async_trait]
impl EventHandler for LoopController {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::PlanCreated,
        ]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext)
        -> Result<Vec<AlephEvent>>
    {
        match event {
            AlephEvent::ToolCallCompleted(result) => {
                // 1. Check guard conditions
                if let Some(stop) = self.check_guards(ctx).await? {
                    return Ok(vec![AlephEvent::LoopStop(stop)]);
                }

                // 2. Send result to LLM, get next step decision
                let decision = self.ask_llm_next_step(result, ctx).await?;

                // 3. Publish events based on LLM decision
                match decision {
                    Decision::CallTool(req) =>
                        Ok(vec![AlephEvent::ToolCallRequested(req)]),
                    Decision::Stop(reason) =>
                        Ok(vec![AlephEvent::LoopStop(reason)]),
                    Decision::AskUser(question) =>
                        Ok(vec![AlephEvent::UserQuestionAsked(question)]),
                }
            }
            AlephEvent::PlanCreated(plan) => {
                // Start executing the first step of the plan
                if let Some(first_step) = plan.next_executable_step() {
                    Ok(vec![AlephEvent::ToolCallRequested(
                        first_step.to_tool_call_request()
                    )])
                } else {
                    Ok(vec![AlephEvent::LoopStop(StopReason::EmptyPlan)])
                }
            }
            _ => Ok(vec![])
        }
    }
}
```

### Protection Mechanisms

```rust
impl LoopController {
    async fn check_guards(&self, ctx: &EventContext) -> Result<Option<StopReason>> {
        let session = ctx.session.read().await;

        // 1. Max iterations
        if session.iteration_count >= self.config.max_iterations {
            return Ok(Some(StopReason::MaxIterationsReached));
        }

        // 2. Doom loop detection
        if self.detect_doom_loop(&session.recent_calls)? {
            return Ok(Some(StopReason::DoomLoopDetected));
        }

        // 3. Token overflow detection
        if session.total_tokens >= self.config.max_tokens_per_session {
            return Ok(Some(StopReason::TokenLimitReached));
        }

        // 4. Check abort signal
        if ctx.abort_signal.load(Ordering::Relaxed) {
            return Ok(Some(StopReason::UserAborted));
        }

        Ok(None)
    }

    fn detect_doom_loop(&self, recent: &[ToolCallRecord]) -> Result<bool> {
        let threshold = self.config.doom_loop_threshold as usize;
        if recent.len() < threshold {
            return Ok(false);
        }

        let last_n = &recent[recent.len() - threshold..];
        let first = &last_n[0];
        Ok(last_n.iter().all(|c| c.tool == first.tool && c.input == first.input))
    }
}

pub enum StopReason {
    Completed,              // Task completed normally
    MaxIterationsReached,   // Hit iteration limit
    DoomLoopDetected,       // Detected infinite loop
    TokenLimitReached,      // Context overflow
    UserAborted,            // User cancelled
    Error(String),          // Unrecoverable error
    EmptyPlan,              // No steps to execute
}
```

---

## Part 4: Sub-agent System

### Agent Definition and Registration

```rust
// Agent definition
pub struct AgentDef {
    pub id: String,                    // "explore", "coder", "researcher"
    pub mode: AgentMode,               // Primary / SubAgent
    pub system_prompt: String,         // System prompt
    pub allowed_tools: Vec<String>,    // Allowed tools
    pub denied_tools: Vec<String>,     // Denied tools
    pub max_iterations: Option<u32>,   // Override default loop limit
}

pub enum AgentMode {
    Primary,    // Main agent, responds directly to user
    SubAgent,   // Sub-agent, called by other agents
}

// Built-in agents
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            id: "main".into(),
            mode: AgentMode::Primary,
            system_prompt: include_str!("prompts/main.md").into(),
            allowed_tools: vec!["*".into()],
            denied_tools: vec![],
            max_iterations: None,
        },
        AgentDef {
            id: "explore".into(),
            mode: AgentMode::SubAgent,
            system_prompt: include_str!("prompts/explore.md").into(),
            allowed_tools: vec![
                "glob", "grep", "read_file", "web_fetch", "search"
            ].into_iter().map(Into::into).collect(),
            denied_tools: vec![
                "write_file", "edit_file", "bash"
            ].into_iter().map(Into::into).collect(),
            max_iterations: Some(20),
        },
        AgentDef {
            id: "coder".into(),
            mode: AgentMode::SubAgent,
            system_prompt: include_str!("prompts/coder.md").into(),
            allowed_tools: vec![
                "read_file", "write_file", "edit_file", "glob", "grep"
            ].into_iter().map(Into::into).collect(),
            denied_tools: vec![],
            max_iterations: Some(30),
        },
        AgentDef {
            id: "researcher".into(),
            mode: AgentMode::SubAgent,
            system_prompt: include_str!("prompts/researcher.md").into(),
            allowed_tools: vec![
                "search", "web_fetch", "read_file"
            ].into_iter().map(Into::into).collect(),
            denied_tools: vec![
                "write_file", "edit_file", "bash"
            ].into_iter().map(Into::into).collect(),
            max_iterations: Some(15),
        },
    ]
}
```

### TaskTool - Tool for Calling Sub-agents

```rust
pub struct TaskTool {
    agent_registry: Arc<AgentRegistry>,
}

impl AgentTool for TaskTool {
    fn name(&self) -> &str { "task" }

    fn description(&self) -> &str {
        "Run a task with a specialized sub-agent. Available agents: explore, coder, researcher"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "enum": ["explore", "coder", "researcher"],
                    "description": "The agent to use"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task to perform"
                }
            },
            "required": ["agent", "prompt"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let agent_id = args["agent"].as_str().unwrap();
        let prompt = args["prompt"].as_str().unwrap();

        // Create child session
        let child_session = ctx.session.fork(agent_id).await?;

        // Publish event to start sub-agent loop
        ctx.bus.publish(AlephEvent::SubAgentStarted(SubAgentRequest {
            agent_id: agent_id.into(),
            prompt: prompt.into(),
            parent_session_id: ctx.session.id.clone(),
            child_session_id: child_session.id.clone(),
        })).await?;

        // Wait for sub-agent completion
        let result = child_session.wait_completion().await?;

        Ok(ToolOutput::text(result.summary))
    }
}
```

---

## Part 5: Session State Persistence

### Execution Session Structure

```rust
// Full execution session
pub struct ExecutionSession {
    pub id: String,
    pub parent_id: Option<String>,      // Parent session ID for child sessions
    pub agent_id: String,               // Current agent being used
    pub status: SessionStatus,

    // Execution state
    pub iteration_count: u32,
    pub total_tokens: u64,
    pub parts: Vec<SessionPart>,        // All execution records
    pub recent_calls: Vec<ToolCallRecord>, // For doom loop detection

    // Model info
    pub model: String,

    // Timestamps
    pub created_at: i64,
    pub updated_at: i64,
}

pub enum SessionStatus {
    Running,
    Completed,
    Failed(String),
    Paused,       // User interrupted
    Compacting,   // Being compacted
}

// Session part - fine-grained records
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),     // Chain of thought
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),         // Compacted summary
}

// Tool call record
pub struct ToolCallPart {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub token_usage: TokenUsage,
}

pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
}

pub struct ToolCallRecord {
    pub tool: String,
    pub input: Value,
    pub timestamp: i64,
}
```

### SQLite Storage Layer

```rust
pub struct SessionStore {
    pool: SqlitePool,
}

impl SessionStore {
    pub async fn init(db_path: &Path) -> Result<Self> {
        let pool = SqlitePool::connect(&format!("sqlite:{}", db_path.display())).await?;

        // Create table structure
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL,
                model TEXT NOT NULL,
                iteration_count INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_parts (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                part_type TEXT NOT NULL,
                part_data TEXT NOT NULL,  -- JSON serialized
                sequence INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_parts_session
                ON session_parts(session_id, sequence);

            CREATE INDEX IF NOT EXISTS idx_sessions_parent
                ON sessions(parent_id);
        "#).execute(&pool).await?;

        Ok(Self { pool })
    }

    // Append session part (incremental write)
    pub async fn append_part(&self, session_id: &str, part: &SessionPart) -> Result<()> {
        let part_type = part.type_name();
        let part_data = serde_json::to_string(part)?;
        let sequence = self.next_sequence(session_id).await?;

        sqlx::query(
            "INSERT INTO session_parts (id, session_id, part_type, part_data, sequence, created_at)
             VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(session_id)
        .bind(part_type)
        .bind(part_data)
        .bind(sequence)
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // Load session for resume
    pub async fn load_session(&self, session_id: &str) -> Result<ExecutionSession> {
        let row = sqlx::query(
            "SELECT * FROM sessions WHERE id = ?"
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;

        let parts = sqlx::query(
            "SELECT * FROM session_parts WHERE session_id = ? ORDER BY sequence"
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        // Reconstruct ExecutionSession from rows
        Ok(ExecutionSession::from_db(row, parts)?)
    }

    // Fork session for sub-agent
    pub async fn fork_session(&self, parent_id: &str, agent_id: &str) -> Result<ExecutionSession> {
        let new_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            "INSERT INTO sessions (id, parent_id, agent_id, status, model, created_at, updated_at)
             VALUES (?, ?, ?, 'Running',
                     (SELECT model FROM sessions WHERE id = ?), ?, ?)"
        )
        .bind(&new_id)
        .bind(parent_id)
        .bind(agent_id)
        .bind(parent_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        self.load_session(&new_id).await
    }
}
```

### SessionRecorder Event Handler

```rust
pub struct SessionRecorder {
    store: Arc<SessionStore>,
}

#[async_trait]
impl EventHandler for SessionRecorder {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::All]  // Subscribe to all events
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        let session_id = ctx.session.read().await.id.clone();

        // Convert event to SessionPart and persist
        if let Some(part) = self.event_to_part(event) {
            self.store.append_part(&session_id, &part).await?;

            // Update session metadata
            self.update_session_metadata(&session_id, event).await?;
        }

        // SessionRecorder doesn't produce new events
        Ok(vec![])
    }
}

impl SessionRecorder {
    fn event_to_part(&self, event: &AlephEvent) -> Option<SessionPart> {
        match event {
            AlephEvent::InputReceived(input) => Some(SessionPart::UserInput(UserInputPart {
                text: input.text.clone(),
                context: input.context.clone(),
                timestamp: input.timestamp,
            })),
            AlephEvent::ToolCallCompleted(result) => Some(SessionPart::ToolCall(ToolCallPart {
                id: result.call_id.clone(),
                tool_name: result.tool.clone(),
                input: result.input.clone(),
                status: ToolCallStatus::Completed,
                output: Some(result.output.to_string()),
                error: None,
                started_at: result.started_at,
                completed_at: Some(result.completed_at),
                token_usage: result.token_usage.clone(),
            })),
            AlephEvent::AiResponseGenerated(response) => Some(SessionPart::AiResponse(AiResponsePart {
                content: response.content.clone(),
                reasoning: response.reasoning.clone(),
                timestamp: response.timestamp,
            })),
            AlephEvent::PlanCreated(plan) => Some(SessionPart::PlanCreated(PlanPart {
                plan: plan.clone(),
                timestamp: chrono::Utc::now().timestamp(),
            })),
            _ => None,
        }
    }
}
```

---

## Part 6: Smart Planning Trigger

### IntentAnalyzer - Determining if Planning is Needed

```rust
pub struct IntentAnalyzer {
    classifier: IntentClassifier,      // Reuse existing 3-layer classification
    complexity_detector: ComplexityDetector,
}

pub struct ComplexityDetector {
    // Complexity judgment thresholds
    multi_step_keywords_zh: Vec<String>,  // "然后", "接着", "并且", "同时"
    multi_step_keywords_en: Vec<String>,  // "then", "after that", "and also"
    high_complexity_intents: Vec<String>, // Intent types requiring planning
}

pub enum Complexity {
    Simple,     // Direct execution
    NeedsPlan,  // Requires planning phase
}

#[async_trait]
impl EventHandler for IntentAnalyzer {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::InputReceived]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        let AlephEvent::InputReceived(input) = event else { return Ok(vec![]) };

        // 1. Use existing classifier to get intent
        let intent = self.classifier.classify(&input.text).await?;

        // 2. Determine complexity
        let complexity = self.analyze_complexity(&input.text, &intent).await?;

        // 3. Decide between planning or direct execution based on complexity
        match complexity {
            Complexity::Simple => {
                // Simple request: build tool call directly
                Ok(vec![AlephEvent::ToolCallRequested(
                    self.build_direct_call(&intent, &input).await?
                )])
            }
            Complexity::NeedsPlan => {
                // Complex request: enter planning phase
                Ok(vec![AlephEvent::PlanRequested(PlanRequest {
                    input: input.clone(),
                    intent,
                    detected_steps: self.extract_steps(&input.text).await?,
                })])
            }
        }
    }
}

impl IntentAnalyzer {
    async fn analyze_complexity(&self, text: &str, intent: &ExecutionIntent) -> Result<Complexity> {
        // Rule 1: Contains multi-step keywords
        if self.complexity_detector.has_multi_step_keywords(text) {
            return Ok(Complexity::NeedsPlan);
        }

        // Rule 2: Intent type is inherently complex
        if self.complexity_detector.is_complex_intent(intent) {
            return Ok(Complexity::NeedsPlan);
        }

        // Rule 3: Ambiguous intent, needs AI decomposition
        if matches!(intent, ExecutionIntent::Ambiguous(_)) {
            return Ok(Complexity::NeedsPlan);
        }

        // Rule 4: Use L3 AI judgment (optional, adds latency)
        if self.should_use_ai_detection(text) {
            return self.ai_complexity_check(text).await;
        }

        Ok(Complexity::Simple)
    }

    fn extract_steps(&self, text: &str) -> Result<Vec<String>> {
        // Simple step extraction using keywords
        let separators = ["然后", "接着", "之后", "并且", "同时",
                          "then", "after that", "and then", "also"];

        let mut steps = vec![];
        let mut current = text.to_string();

        for sep in &separators {
            if current.contains(sep) {
                let parts: Vec<_> = current.split(sep).collect();
                if parts.len() > 1 {
                    steps.push(parts[0].trim().to_string());
                    current = parts[1..].join(sep);
                }
            }
        }

        if !current.is_empty() {
            steps.push(current.trim().to_string());
        }

        Ok(steps)
    }
}
```

### TaskPlanner - Task Planner

```rust
pub struct TaskPlanner {
    llm_client: Arc<dyn LlmProvider>,
}

#[async_trait]
impl EventHandler for TaskPlanner {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::PlanRequested]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        let AlephEvent::PlanRequested(request) = event else { return Ok(vec![]) };

        // Get available tools list
        let tools = ctx.tools.list_available();

        // Call LLM to generate task plan
        let plan = self.generate_plan(&request, &tools).await?;

        Ok(vec![AlephEvent::PlanCreated(plan)])
    }
}

impl TaskPlanner {
    async fn generate_plan(&self, request: &PlanRequest, tools: &[UnifiedTool]) -> Result<TaskPlan> {
        let prompt = format!(r#"
You are a task planner. Please decompose the user request into executable steps.

User request: {}
Detected intent: {:?}
Pre-extracted steps: {:?}

Available tools:
{}

Please generate a task plan in JSON format:
{{
  "steps": [
    {{
      "id": "step_1",
      "description": "Step description",
      "tool": "tool_name",
      "parameters": {{}},
      "depends_on": []  // IDs of prerequisite steps
    }}
  ],
  "parallel_groups": [["step_2", "step_3"]]  // Steps that can run in parallel
}}

Rules:
1. Each step should use exactly one tool
2. Specify dependencies correctly - a step can only depend on earlier steps
3. Group independent steps for parallel execution
4. Keep the plan minimal - don't add unnecessary steps
"#, request.input.text, request.intent, request.detected_steps, self.format_tools(tools));

        let response = self.llm_client.complete(&prompt).await?;
        let plan: TaskPlan = serde_json::from_str(&response)?;

        Ok(TaskPlan {
            id: uuid::Uuid::new_v4().to_string(),
            ..plan
        })
    }

    fn format_tools(&self, tools: &[UnifiedTool]) -> String {
        tools.iter()
            .map(|t| format!("- {}: {}", t.name, t.description.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// Task plan structure
pub struct TaskPlan {
    pub id: String,
    pub steps: Vec<PlanStep>,
    pub parallel_groups: Vec<Vec<String>>,  // Steps that can run in parallel
    pub current_step_index: usize,
}

pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub tool: String,
    pub parameters: Value,
    pub depends_on: Vec<String>,
    pub status: StepStatus,
}

pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

impl TaskPlan {
    pub fn next_executable_step(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| {
            matches!(s.status, StepStatus::Pending) &&
            s.depends_on.iter().all(|dep_id| {
                self.steps.iter()
                    .find(|d| &d.id == dep_id)
                    .map(|d| matches!(d.status, StepStatus::Completed))
                    .unwrap_or(false)
            })
        })
    }
}
```

---

## Part 7: Tool Execution and Retry Mechanism

### ToolExecutor - Unified Tool Executor

```rust
pub struct ToolExecutor {
    registry: Arc<ToolRegistry>,
    retry_policy: RetryPolicy,
}

pub struct RetryPolicy {
    pub max_retries: u32,              // Max retry count, default 3
    pub base_delay_ms: u64,            // Base delay, default 1000ms
    pub max_delay_ms: u64,             // Max delay, default 30000ms
    pub retryable_errors: Vec<ErrorKind>,  // Retryable error types
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
            retryable_errors: vec![
                ErrorKind::Timeout,
                ErrorKind::RateLimit,
                ErrorKind::ServiceUnavailable,
            ],
        }
    }
}

#[async_trait]
impl EventHandler for ToolExecutor {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallRequested]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        let AlephEvent::ToolCallRequested(request) = event else { return Ok(vec![]) };

        // Record tool call start
        let call_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now().timestamp();

        ctx.bus.publish(AlephEvent::ToolCallStarted(ToolCallStarted {
            call_id: call_id.clone(),
            tool: request.tool.clone(),
            input: request.parameters.clone(),
            timestamp: started_at,
        })).await?;

        // Execute tool (with retry)
        let result = self.execute_with_retry(&request, &call_id, started_at, ctx).await;

        // Publish result event
        match result {
            Ok(output) => Ok(vec![AlephEvent::ToolCallCompleted(ToolCallResult {
                call_id,
                tool: request.tool.clone(),
                input: request.parameters.clone(),
                output,
                started_at,
                completed_at: chrono::Utc::now().timestamp(),
                token_usage: TokenUsage::default(),
            })]),
            Err(e) => Ok(vec![AlephEvent::ToolCallFailed(ToolCallError {
                call_id,
                tool: request.tool.clone(),
                error: e.to_string(),
                error_kind: e.kind(),
                is_retryable: self.is_retryable(&e),
                attempts: e.attempts(),
            })]),
        }
    }
}

impl ToolExecutor {
    async fn execute_with_retry(
        &self,
        request: &ToolCallRequest,
        call_id: &str,
        started_at: i64,
        ctx: &EventContext,
    ) -> Result<ToolOutput, ToolError> {
        let mut attempts = 0;
        let mut last_error = None;

        while attempts <= self.retry_policy.max_retries {
            if attempts > 0 {
                // Exponential backoff
                let delay = self.calculate_delay(attempts);
                ctx.bus.publish(AlephEvent::ToolCallRetrying(ToolCallRetry {
                    call_id: call_id.into(),
                    attempt: attempts,
                    delay_ms: delay,
                    reason: last_error.as_ref().map(|e: &ToolError| e.to_string()),
                })).await.ok();
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }

            // Check abort signal before each attempt
            if ctx.abort_signal.load(Ordering::Relaxed) {
                return Err(ToolError::aborted());
            }

            match self.execute_once(request, ctx).await {
                Ok(output) => return Ok(output),
                Err(e) if self.is_retryable(&e) && attempts < self.retry_policy.max_retries => {
                    last_error = Some(e);
                    attempts += 1;
                }
                Err(e) => return Err(e.with_attempts(attempts)),
            }
        }

        Err(last_error.unwrap().with_attempts(attempts))
    }

    fn calculate_delay(&self, attempt: u32) -> u64 {
        let delay = self.retry_policy.base_delay_ms * 2u64.pow(attempt - 1);
        delay.min(self.retry_policy.max_delay_ms)
    }

    fn is_retryable(&self, error: &ToolError) -> bool {
        self.retry_policy.retryable_errors.contains(&error.kind())
    }

    async fn execute_once(&self, request: &ToolCallRequest, ctx: &EventContext) -> Result<ToolOutput, ToolError> {
        // Find tool
        let tool = self.registry.get(&request.tool)
            .ok_or_else(|| ToolError::not_found(&request.tool))?;

        // Check permission
        self.check_permission(&tool, ctx).await?;

        // Execute tool
        let tool_ctx = ToolContext {
            session: Arc::clone(&ctx.session),
            bus: ctx.bus.clone(),
            abort_signal: Arc::clone(&ctx.abort_signal),
        };

        tool.execute(request.parameters.clone(), &tool_ctx).await
    }
}
```

### Error Types

```rust
#[derive(Debug)]
pub struct ToolError {
    kind: ErrorKind,
    message: String,
    attempts: u32,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    NotFound,           // Tool doesn't exist
    InvalidInput,       // Parameter error
    PermissionDenied,   // Permission denied
    Timeout,            // Execution timeout
    RateLimit,          // Rate limited
    ServiceUnavailable, // Service unavailable
    ExecutionFailed,    // Execution failed
    Aborted,            // User aborted
}

impl ToolError {
    pub fn not_found(tool: &str) -> Self {
        Self {
            kind: ErrorKind::NotFound,
            message: format!("Tool '{}' not found", tool),
            attempts: 0,
            source: None,
        }
    }

    pub fn aborted() -> Self {
        Self {
            kind: ErrorKind::Aborted,
            message: "Operation aborted by user".into(),
            attempts: 0,
            source: None,
        }
    }

    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind.clone()
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}
```

---

## Part 8: Session Compaction and Token Management

### TokenTracker - Token Usage Tracking

```rust
pub struct TokenTracker {
    model_limits: HashMap<String, ModelLimit>,
}

pub struct ModelLimit {
    pub context_limit: u64,       // Context window size
    pub max_output_tokens: u64,   // Max output tokens
    pub reserve_ratio: f32,       // Reserve ratio, default 0.2
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context_limit: 128000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        }
    }
}

impl TokenTracker {
    pub fn new() -> Self {
        let mut model_limits = HashMap::new();

        // Claude models
        model_limits.insert("claude-3-opus".into(), ModelLimit {
            context_limit: 200000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        });
        model_limits.insert("claude-3-sonnet".into(), ModelLimit {
            context_limit: 200000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        });

        // GPT models
        model_limits.insert("gpt-4-turbo".into(), ModelLimit {
            context_limit: 128000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        });

        Self { model_limits }
    }

    pub fn is_overflow(&self, session: &ExecutionSession, model: &str) -> bool {
        let limit = self.model_limits.get(model).unwrap_or(&ModelLimit::default());
        let usable = limit.context_limit - limit.max_output_tokens;
        let threshold = (usable as f32 * (1.0 - limit.reserve_ratio)) as u64;

        session.total_tokens > threshold
    }

    pub fn estimate_tokens(&self, text: &str) -> u64 {
        // Simple estimation: ~0.5 token/char for CJK, ~0.25 for English
        let chars = text.chars().count() as u64;
        (chars as f64 * 0.4) as u64
    }
}
```

### SessionCompactor - Session Compressor

```rust
pub struct SessionCompactor {
    llm_client: Arc<dyn LlmProvider>,
    token_tracker: TokenTracker,
}

#[async_trait]
impl EventHandler for SessionCompactor {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallCompleted, EventType::LoopContinue]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        let session = ctx.session.read().await;
        let model = session.model.clone();
        let session_id = session.id.clone();
        let tokens_before = session.total_tokens;

        // Check if compaction is needed
        if !self.token_tracker.is_overflow(&session, &model) {
            return Ok(vec![]);
        }

        drop(session); // Release read lock

        // Perform compaction
        self.compact(ctx).await?;

        let tokens_after = ctx.session.read().await.total_tokens;

        Ok(vec![AlephEvent::SessionCompacted(CompactionInfo {
            session_id,
            tokens_before,
            tokens_after,
            timestamp: chrono::Utc::now().timestamp(),
        })])
    }
}

impl SessionCompactor {
    async fn compact(&self, ctx: &EventContext) -> Result<()> {
        let mut session = ctx.session.write().await;
        let model = session.model.clone();

        // Strategy 1: Prune old tool outputs (keep recent N)
        let keep_recent = 10;
        self.prune_old_tool_outputs(&mut session, keep_recent)?;

        // Strategy 2: If still overflowing, generate summary to replace history
        if self.token_tracker.is_overflow(&session, &model) {
            drop(session); // Release for LLM call
            let summary = self.generate_summary(&ctx.session.read().await).await?;
            let mut session = ctx.session.write().await;
            self.replace_with_summary(&mut session, summary)?;
        }

        Ok(())
    }

    fn prune_old_tool_outputs(&self, session: &mut ExecutionSession, keep_recent: usize) -> Result<()> {
        let tool_indices: Vec<_> = session.parts.iter()
            .enumerate()
            .filter(|(_, p)| matches!(p, SessionPart::ToolCall(_)))
            .map(|(i, _)| i)
            .collect();

        if tool_indices.len() <= keep_recent {
            return Ok(());
        }

        // Truncate old tool outputs
        let to_prune = tool_indices.len() - keep_recent;
        for idx in tool_indices.into_iter().take(to_prune) {
            if let SessionPart::ToolCall(ref mut tc) = session.parts[idx] {
                tc.output = Some("[Output pruned to save context]".into());
            }
        }

        // Recalculate tokens
        session.total_tokens = self.recalculate_tokens(session);

        Ok(())
    }

    async fn generate_summary(&self, session: &ExecutionSession) -> Result<String> {
        let history = self.format_history_for_summary(session);

        let prompt = format!(r#"
Please compress the following conversation history into a concise summary, preserving key information:
1. User's original request
2. Important completed steps
3. Current progress and pending tasks
4. Key intermediate results

Conversation history:
{}

Please generate a summary (max 500 words):
"#, history);

        self.llm_client.complete(&prompt).await
    }

    fn replace_with_summary(&self, session: &mut ExecutionSession, summary: String) -> Result<()> {
        // Keep last few parts
        let keep_last = 5;
        let recent: Vec<_> = session.parts.drain(session.parts.len().saturating_sub(keep_last)..).collect();

        // Clear old history, insert summary
        session.parts.clear();
        session.parts.push(SessionPart::Summary(SummaryPart {
            content: summary,
            original_count: session.iteration_count,
            compacted_at: chrono::Utc::now().timestamp(),
        }));

        // Restore recent parts
        session.parts.extend(recent);

        // Recalculate tokens
        session.total_tokens = self.recalculate_tokens(session);

        Ok(())
    }

    fn recalculate_tokens(&self, session: &ExecutionSession) -> u64 {
        session.parts.iter().map(|p| {
            match p {
                SessionPart::UserInput(u) => self.token_tracker.estimate_tokens(&u.text),
                SessionPart::AiResponse(a) => self.token_tracker.estimate_tokens(&a.content),
                SessionPart::ToolCall(t) => {
                    self.token_tracker.estimate_tokens(&t.input.to_string()) +
                    t.output.as_ref().map(|o| self.token_tracker.estimate_tokens(o)).unwrap_or(0)
                }
                SessionPart::Summary(s) => self.token_tracker.estimate_tokens(&s.content),
                _ => 0,
            }
        }).sum()
    }

    fn format_history_for_summary(&self, session: &ExecutionSession) -> String {
        session.parts.iter().filter_map(|p| {
            match p {
                SessionPart::UserInput(u) => Some(format!("User: {}", u.text)),
                SessionPart::AiResponse(a) => Some(format!("AI: {}", a.content)),
                SessionPart::ToolCall(t) => Some(format!("Tool {}: {} -> {}",
                    t.tool_name, t.input, t.output.as_deref().unwrap_or("(pending)"))),
                _ => None,
            }
        }).collect::<Vec<_>>().join("\n\n")
    }
}
```

---

## Part 9: Integration with Existing Code

### FFI Layer Refactoring

```rust
// Existing entry - maintain API compatibility
impl AlephCore {
    pub fn process(&self, input: String, options: Option<ProcessOptions>) {
        let runtime = self.runtime.clone();
        let bus = self.event_bus.clone();

        runtime.spawn(async move {
            // Publish input event, starting the entire event chain
            bus.publish(AlephEvent::InputReceived(InputEvent {
                text: input,
                topic_id: options.and_then(|o| o.topic_id),
                context: CapturedContext::current(),
                timestamp: chrono::Utc::now().timestamp(),
            })).await.ok();
        });
    }

    // New: Resume session
    pub fn resume_session(&self, session_id: String) {
        let runtime = self.runtime.clone();
        let bus = self.event_bus.clone();
        let store = self.session_store.clone();

        runtime.spawn(async move {
            if let Ok(session) = store.load_session(&session_id).await {
                bus.publish(AlephEvent::SessionResumed(session)).await.ok();
            }
        });
    }

    // New: Cancel current session
    pub fn cancel(&self) {
        self.abort_signal.store(true, Ordering::Relaxed);
    }
}

// AlephCore initialization
impl AlephCore {
    pub async fn new(config: AlephConfig, callback: Arc<dyn AlephEventCallback>) -> Result<Self> {
        // Create event bus
        let event_bus = EventBus::new(1024);

        // Create shared context
        let session_store = Arc::new(SessionStore::init(&config.db_path).await?);
        let tool_registry = Arc::new(ToolRegistry::new(&config).await?);
        let abort_signal = Arc::new(AtomicBool::new(false));

        // Create initial session
        let initial_session = Arc::new(RwLock::new(ExecutionSession::new()));

        // Create event context
        let ctx = Arc::new(EventContext {
            session: initial_session,
            config: Arc::new(config.clone()),
            tools: tool_registry.clone(),
            bus: event_bus.clone(),
            abort_signal: abort_signal.clone(),
        });

        // Register all event handlers
        let mut registry = ComponentRegistry::new();

        registry.register(Arc::new(IntentAnalyzer::new(config.clone())));
        registry.register(Arc::new(TaskPlanner::new(config.llm.clone())));
        registry.register(Arc::new(ToolExecutor::new(tool_registry.clone())));
        registry.register(Arc::new(LoopController::new(config.loop_config.clone())));
        registry.register(Arc::new(SessionRecorder::new(session_store.clone())));
        registry.register(Arc::new(SessionCompactor::new(config.llm.clone())));
        registry.register(Arc::new(CallbackBridge::new(callback)));

        // Start event listening
        registry.start(&event_bus, ctx).await;

        Ok(Self {
            config,
            event_bus,
            session_store,
            tool_registry,
            component_registry: registry,
            abort_signal,
            runtime: tokio::runtime::Handle::current(),
        })
    }
}
```

### Callback Interface - Notify Swift Layer

```rust
// UniFFI callback definition
#[uniffi::export(callback_interface)]
pub trait AlephEventCallback: Send + Sync {
    fn on_session_started(&self, session_id: String);
    fn on_tool_call_started(&self, call_id: String, tool: String);
    fn on_tool_call_completed(&self, call_id: String, output: String);
    fn on_tool_call_failed(&self, call_id: String, error: String, is_retryable: bool);
    fn on_progress_update(&self, session_id: String, iteration: u32, status: String);
    fn on_response(&self, session_id: String, text: String, is_final: bool);
    fn on_error(&self, session_id: String, error: String);
    fn on_session_completed(&self, session_id: String, summary: String);
    fn on_plan_created(&self, session_id: String, steps: Vec<String>);
}

// Event forwarder - convert internal events to callbacks
pub struct CallbackBridge {
    callback: Arc<dyn AlephEventCallback>,
}

impl CallbackBridge {
    pub fn new(callback: Arc<dyn AlephEventCallback>) -> Self {
        Self { callback }
    }
}

#[async_trait]
impl EventHandler for CallbackBridge {
    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::SessionCreated,
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
            EventType::LoopContinue,
            EventType::AiResponseGenerated,
            EventType::LoopStop,
            EventType::PlanCreated,
        ]
    }

    async fn handle(&self, event: &AlephEvent, ctx: &EventContext) -> Result<Vec<AlephEvent>> {
        match event {
            AlephEvent::SessionCreated(info) => {
                self.callback.on_session_started(info.id.clone());
            }
            AlephEvent::ToolCallStarted(info) => {
                self.callback.on_tool_call_started(
                    info.call_id.clone(),
                    info.tool.clone()
                );
            }
            AlephEvent::ToolCallCompleted(result) => {
                self.callback.on_tool_call_completed(
                    result.call_id.clone(),
                    result.output.to_string()
                );
            }
            AlephEvent::ToolCallFailed(error) => {
                self.callback.on_tool_call_failed(
                    error.call_id.clone(),
                    error.error.clone(),
                    error.is_retryable
                );
            }
            AlephEvent::LoopContinue(state) => {
                let session = ctx.session.read().await;
                self.callback.on_progress_update(
                    session.id.clone(),
                    session.iteration_count,
                    format!("{:?}", state)
                );
            }
            AlephEvent::AiResponseGenerated(response) => {
                let session_id = ctx.session.read().await.id.clone();
                self.callback.on_response(
                    session_id,
                    response.content.clone(),
                    response.is_final
                );
            }
            AlephEvent::LoopStop(reason) => {
                let session = ctx.session.read().await;
                match reason {
                    StopReason::Completed => {
                        self.callback.on_session_completed(
                            session.id.clone(),
                            "Task completed successfully".into()
                        );
                    }
                    StopReason::Error(e) => {
                        self.callback.on_error(session.id.clone(), e.clone());
                    }
                    _ => {
                        self.callback.on_session_completed(
                            session.id.clone(),
                            format!("Stopped: {:?}", reason)
                        );
                    }
                }
            }
            AlephEvent::PlanCreated(plan) => {
                let session_id = ctx.session.read().await.id.clone();
                let steps: Vec<String> = plan.steps.iter()
                    .map(|s| s.description.clone())
                    .collect();
                self.callback.on_plan_created(session_id, steps);
            }
            _ => {}
        }

        Ok(vec![])
    }
}
```

### Module Retention Strategy

```
Existing modules retention strategy:

✅ Retain and reuse:
  - intent/classifier.rs      → Used internally by IntentAnalyzer
  - intent/keyword.rs         → Used internally by IntentAnalyzer
  - dispatcher/registry.rs    → Used internally by ToolExecutor
  - dispatcher/types.rs       → Global type definitions
  - rig_tools/*               → Tool implementations unchanged
  - mcp/*                     → MCP client unchanged
  - memory/*                  → Continue as tool

🔄 Refactor:
  - ffi/processing.rs         → Change to event publishing entry
  - agent/manager.rs          → Split into LoopController + ToolExecutor
  - conversation/session.rs   → Migrate to ExecutionSession

❌ Deprecate:
  - cowork/scheduler/*        → Replaced by event-driven
  - cowork/executor/*         → Replaced by ToolExecutor
```

---

## Part 10: Overall Architecture and Implementation Roadmap

### Complete Event Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              EventBus (broadcast channel)                    │
└─────────────────────────────────────────────────────────────────────────────┘
       ↑↓                ↑↓                ↑↓                ↑↓           ↑↓
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌────────────┐  ┌────────────┐
│IntentAnalyzer│  │ TaskPlanner  │  │ ToolExecutor │  │LoopControl │  │SessionRec. │
├──────────────┤  ├──────────────┤  ├──────────────┤  ├────────────┤  ├────────────┤
│Subscribe:    │  │Subscribe:    │  │Subscribe:    │  │Subscribe:  │  │Subscribe:  │
│InputReceived │  │PlanRequested │  │ToolCallReq.  │  │ToolCallRes.│  │ All Events │
├──────────────┤  ├──────────────┤  ├──────────────┤  ├────────────┤  ├────────────┤
│Publish:      │  │Publish:      │  │Publish:      │  │Publish:    │  │Publish:    │
│PlanRequested │  │PlanCreated   │  │ToolCallRes.  │  │LoopContinue│  │(none)      │
│ToolCallReq.  │  │              │  │ToolCallFail  │  │LoopStop    │  │            │
└──────────────┘  └──────────────┘  └──────────────┘  └────────────┘  └────────────┘
                                                              ↓
                                                     ┌────────────────┐
                                                     │SessionCompactor│
                                                     ├────────────────┤
                                                     │Sub:LoopContinue│
                                                     │Pub:Compacted   │
                                                     └────────────────┘
```

### Typical Request Flow

```
User: "Search for Rust async best practices, then write an example code"

1. InputReceived ─────────────────────────────────────→ IntentAnalyzer

2. IntentAnalyzer detects multi-step keyword "then"
   PlanRequested ─────────────────────────────────────→ TaskPlanner

3. TaskPlanner generates plan:
   Step1: search("Rust async best practices")
   Step2: write_file("example.rs", depends_on: [Step1])
   PlanCreated ───────────────────────────────────────→ LoopController

4. LoopController starts first step
   ToolCallRequested(search) ─────────────────────────→ ToolExecutor

5. ToolExecutor executes search
   ToolCallCompleted(search_results) ─────────────────→ LoopController

6. LoopController asks LLM for next step
   LLM: "Call write_file tool"
   ToolCallRequested(write_file) ─────────────────────→ ToolExecutor

7. ToolExecutor executes file write
   ToolCallCompleted(file_written) ───────────────────→ LoopController

8. LoopController asks LLM
   LLM: "Task completed"
   LoopStop(Completed) ───────────────────────────────→ CallbackBridge

9. CallbackBridge.on_session_completed() ─────────────→ Swift UI
```

### File Structure Plan

```
Aleph/core/src/
├── lib.rs
├── aleph.udl
│
├── event/                      # New: Event system
│   ├── mod.rs
│   ├── bus.rs                  # EventBus implementation
│   ├── types.rs                # AlephEvent enum
│   └── handler.rs              # EventHandler trait
│
├── components/                 # New: Event handler components
│   ├── mod.rs
│   ├── intent_analyzer.rs      # Intent analysis + complexity detection
│   ├── task_planner.rs         # Task planner
│   ├── tool_executor.rs        # Tool execution + retry
│   ├── loop_controller.rs      # Loop control + protection
│   ├── session_recorder.rs     # State persistence
│   ├── session_compactor.rs    # Session compaction
│   └── callback_bridge.rs      # UniFFI callback bridge
│
├── session/                    # New: Session management
│   ├── mod.rs
│   ├── types.rs                # ExecutionSession, SessionPart
│   ├── store.rs                # SQLite storage
│   └── fork.rs                 # Session forking
│
├── agent/                      # Refactor: Sub-agents
│   ├── mod.rs
│   ├── types.rs                # AgentDef, AgentMode
│   ├── registry.rs             # Agent registry
│   ├── builtin.rs              # Built-in agent definitions
│   └── task_tool.rs            # TaskTool implementation
│
├── intent/                     # Keep: Intent detection
├── dispatcher/                 # Keep: Tool registration
├── rig_tools/                  # Keep: Tool implementations
├── mcp/                        # Keep: MCP client
├── memory/                     # Keep: RAG module
└── ffi/                        # Refactor: Event entry
```

### Implementation Roadmap

```
Phase 1: Event Infrastructure (1-2 weeks)
├── event/bus.rs          - EventBus implementation
├── event/types.rs        - Event type definitions
├── event/handler.rs      - EventHandler trait
└── Unit tests

Phase 2: Core Components (2-3 weeks)
├── components/tool_executor.rs     - Tool execution + retry
├── components/loop_controller.rs   - Loop control + protection
├── components/intent_analyzer.rs   - Intent analysis
└── Integrate existing intent/ and dispatcher/

Phase 3: Session Management (1-2 weeks)
├── session/types.rs      - ExecutionSession
├── session/store.rs      - SQLite persistence
├── components/session_recorder.rs
└── components/session_compactor.rs

Phase 4: Planning and Sub-agents (2 weeks)
├── components/task_planner.rs      - Task planning
├── agent/                          - Sub-agent system
└── agent/task_tool.rs              - TaskTool

Phase 5: Integration and Testing (1-2 weeks)
├── ffi/ refactoring
├── components/callback_bridge.rs
├── End-to-end tests
└── Swift UI adaptation
```

---

## Key Design Principles

| Principle | Implementation |
|-----------|---------------|
| **Modularity** | Tool.Info/Agent.Info/Session.Info independent namespaces |
| **Extensibility** | Support custom tools/agents/MCP servers |
| **Permission Isolation** | Three-layer permission system |
| **Async Consistency** | Heavy use of async/await |
| **Event-Driven** | Bus pub-sub for all key state changes |
| **State Management** | Session state caching, SQLite persistence |
| **Error Recovery** | Retry mechanism + abort signal + fork recovery |
| **Performance** | Token calculation/session compaction/output truncation |

## Innovation Points

1. **Session Tree Structure** - Supports fork and multi-step task chains
2. **Partitioned Messages** - TextPart/ToolPart fine-grained processing
3. **Doom Loop Detection** - Auto-detect infinite tool loops
4. **Dynamic Planning** - AI-driven task decomposition
5. **Smart Complexity Detection** - Auto-determine when planning is needed
6. **Type-Safe Event Bus** - Rust enum-based event system

---

## References

- OpenCode project: https://github.com/opencode-ai/opencode
- Claude Code CLI architecture
- Tokio broadcast channel documentation
- SQLite async with sqlx
