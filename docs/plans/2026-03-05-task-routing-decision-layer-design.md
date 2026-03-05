# Task Routing Decision Layer Design

> Date: 2026-03-05
> Status: Approved
> Scope: Insert a routing decision layer between Gateway and execution, enabling tasks to be dispatched to Agent Loop, Dispatcher DAG, POE Full, or Swarm based on complexity
> Predecessor: `docs/reports/2026-03-05-architecture-activation-diagnostic-report.md`

## Problem

Diagnostic analysis (2026-03-05) revealed that Aleph operates as an enhanced stream processor at runtime. Out of 9 architectural subsystems, only 3 are activated (Agent Loop, Thinker, POE Lazy). Dispatcher DAG, POE Full Manager, Swarm, and Resilience are implemented (~60,000 LOC) but never called. The root cause: Agent Loop is a "万能打工者" that handles all tasks internally via think-act loop, never delegating to higher-level architecture.

## Goal

Introduce a `TaskRouter` that decides which execution path a task should take, based on two-phase classification (rules + LLM fallback) and dynamic upgrade from within Agent Loop.

## Core Types

### TaskRoute

```rust
pub enum TaskRoute {
    /// Simple task: direct Agent Loop OTAF
    Simple,

    /// Multi-step task: Dispatcher decomposes into DAG, each node executes via Agent Loop
    MultiStep { reason: String },

    /// Critical task: Dispatcher DAG wrapped by POE Full Manager with SuccessManifest validation
    Critical { reason: String, manifest_hints: ManifestHints },

    /// Collaborative task: Swarm multi-agent parallel/adversarial execution
    Collaborative { reason: String, strategy: CollabStrategy },
}

pub struct ManifestHints {
    pub hard_constraints: Vec<String>,
    pub quality_threshold: f64,
}

pub enum CollabStrategy {
    Parallel,      // Multi-domain parallel (analyze + write docs simultaneously)
    Adversarial,   // Adversarial verification (generate + review)
    GroupChat,     // User-requested multi-persona conversation
}
```

### TaskRouter Trait

```rust
#[async_trait]
pub trait TaskRouter: Send + Sync {
    /// Pre-classification: categorize message before Agent Loop starts
    async fn classify(&self, message: &str, context: &RouterContext) -> TaskRoute;

    /// Dynamic escalation: check if running Agent Loop should upgrade
    async fn should_escalate(&self, state: &EscalationContext) -> Option<TaskRoute>;
}
```

## Phase 1: Pre-Classification (Entry Point)

### Rules Layer (zero latency)

Pattern matching against configurable rules, covering high-confidence scenarios:

```
Message → rule_classify()
            ├─ Some(route) → use directly (zero latency)
            └─ None → llm_classify() → route (~1-2s)
```

Rule categories:
- **Collaborative**: `/group` commands, `@role` mentions
- **Critical**: generation + quality patterns ("生成报告并确保...", "审查代码并修复")
- **MultiStep**: sequential instruction patterns ("先...然后...最后...")
- **Simple**: greetings, single-sentence Q&A, translations

Patterns stored in `~/.aleph/config.toml` under `[routing.patterns]`, hot-reloadable. Expected rule hit rate: 60-70%.

### LLM Fallback (~1-2s)

When rules don't match, use the fastest available model for classification:

```rust
async fn llm_classify(message: &str, context: &RouterContext) -> TaskRoute {
    // Use ModelRouter to select fastest model (prefer local/haiku tier)
    // Single-shot classification prompt returning JSON
    // { "route": "multi_step", "reason": "...", "hints": {...} }
}
```

## Phase 2: Dynamic Escalation (Inside Agent Loop)

### Path 1: LLM Self-Escalation (escalate_task tool)

A built-in tool injected into Agent Loop's available tools:

```rust
pub struct EscalateTaskTool;

// Tool description for LLM:
// "Call this when the current task needs multiple independent steps,
//  strict quality verification, or different expert collaboration."
//
// Parameters:
//   target: "multi_step" | "critical" | "collaborative"
//   reason: why escalation is needed
//   subtasks: optional suggested decomposition
```

When LLM calls this tool, Agent Loop pauses and returns `LoopResult::Escalated`.

### Path 2: Step Threshold Guard (automatic fallback)

New guard in Agent Loop's guard check phase:

```rust
if state.step_count >= ESCALATION_THRESHOLD  // default: 3
    && !state.escalation_checked              // fire once
{
    state.escalation_checked = true;
    if let Some(new_route) = router.should_escalate(&context).await {
        return LoopResult::Escalated { route: new_route, context: state.snapshot() };
    }
}
```

`should_escalate` checks: are tool calls spread across unrelated domains (suggests parallelization), or are there retry failures (suggests POE protection).

### LoopResult Extension

```rust
pub enum LoopResult {
    Completed { ... },
    Failed { ... },
    UserAborted,
    GuardTriggered(GuardViolation),
    PoeAborted { reason: String },
    Escalated { route: TaskRoute, context: EscalationSnapshot },  // NEW
}
```

### Context Preservation on Escalation

```rust
pub struct EscalationSnapshot {
    pub original_message: String,
    pub conversation_history: Vec<Message>,
    pub completed_steps: Vec<LoopStep>,
    pub tools_invoked: Vec<String>,
    pub partial_result: Option<String>,
}
```

Completed steps are passed to downstream executors as "known progress" to avoid duplicate work.

## Route Execution

Gateway ExecutionEngine dispatches based on TaskRoute (no new Orchestrator module):

```rust
match route {
    TaskRoute::Simple => {
        // Existing path, unchanged
        self.run_agent_loop(message, ctx).await
    }

    TaskRoute::MultiStep { .. } => {
        // 1. Planner decomposes into TaskGraph (DAG)
        // 2. DagScheduler executes (each node via Agent Loop)
        // 3. Aggregate results
        self.run_dispatcher_dag(message, ctx).await
    }

    TaskRoute::Critical { manifest_hints, .. } => {
        // 1. Build SuccessManifest from hints
        // 2. PoeManager::execute(PoeTask) wraps entire flow
        // 3. Internal worker uses Dispatcher DAG
        self.run_poe_with_dag(message, manifest_hints, ctx).await
    }

    TaskRoute::Collaborative { strategy, .. } => {
        match strategy {
            Parallel => self.run_swarm_parallel(message, ctx).await,
            Adversarial => self.run_swarm_adversarial(message, ctx).await,
            GroupChat => self.run_group_chat(message, ctx).await,
        }
    }
}
```

Agent Loop escalation handling:

```rust
let result = agent_loop.run().await;
if let LoopResult::Escalated { route, context } = result {
    self.execute_routed_task_with_context(route, context).await
}
```

## Swarm Strategies

### Parallel (multi-domain)

Dispatcher Planner decomposes into independent subtasks. Each subtask assigned to a different Agent via SessionCoordinator. SwarmCoordinator publishes/consumes events for cross-agent awareness. Results aggregated.

### Adversarial (verification)

Two roles: Generator + Reviewer. Generator executes task, Reviewer audits result and provides feedback. If rejected, Generator revises (up to N rounds). POE Full as final validation gate.

### GroupChat (user-requested)

GroupChatOrchestrator creates session with specified personas. Turn-based execution via existing implementation. Results merged and returned.

## User Notification

Lightweight hints via existing channel messaging, non-blocking:

| Route | Message |
|-------|---------|
| Simple | (none) |
| MultiStep | "📋 正在规划多步执行计划..." |
| Critical | "🔍 正在建立质量验证标准..." |
| Collaborative | "👥 正在组织多角色协作..." |

## Graceful Degradation

```
Swarm failure  → fallback to Dispatcher DAG (single agent)
DAG failure    → fallback to Agent Loop (direct execution)
POE over-budget → return best-effort result + warning
```

No user-facing "system error" — always a fallback path.

## Configuration

```toml
[routing]
enable_llm_fallback = true
classify_model = "fast"

escalation_step_threshold = 3
escalation_enabled = true

max_parallel_agents = 4
adversarial_max_rounds = 3

[routing.patterns]
critical = ["生成.*报告", "分析.*并.*生成", "审查.*并修复"]
multi_step = ["先.*然后.*最后", "分步", "依次完成"]
simple = ["你好", "什么是", "帮我翻译"]
collaborative = ["/group", "@专家"]
```

## Architecture Alignment

| Principle | How This Design Complies |
|-----------|--------------------------|
| R1 (Brain-Limb Separation) | Router is pure decision logic in Core, no platform APIs |
| R4 (I/O-Only Interfaces) | Gateway only receives route result, no business logic |
| P1 (Low Coupling) | TaskRouter is a trait; rules, LLM, and execution are independent |
| P3 (Extensibility) | New routes added by extending TaskRoute enum + match arm |
| P4 (Dependency Inversion) | ExecutionEngine depends on TaskRouter trait, not concrete impl |
| P6 (Simplicity) | Rules handle common cases at zero cost; LLM only for ambiguous cases |
| P7 (Defensive Design) | Graceful degradation chain ensures no hard failures |

## Files to Create/Modify

| Action | File | Description |
|--------|------|-------------|
| Create | `core/src/routing/task_router.rs` | TaskRoute, TaskRouter trait, RouterContext, EscalationContext |
| Create | `core/src/routing/rules.rs` | Rule-based classifier with pattern matching |
| Create | `core/src/routing/llm_classifier.rs` | LLM fallback classifier |
| Create | `core/src/routing/composite_router.rs` | CompositeRouter impl combining rules + LLM |
| Create | `core/src/builtin_tools/escalate_task.rs` | EscalateTask built-in tool |
| Modify | `core/src/agent_loop/agent_loop.rs` | Add escalation guard + LoopResult::Escalated |
| Modify | `core/src/gateway/execution_engine/engine.rs` | Route dispatch + escalation handling |
| Modify | `core/src/config/` | Add `[routing]` config section |
