# Multi-Agent System

Aleph provides three user-triggerable collaboration modes plus a shared sensing infrastructure layer. All modes operate through tools (R9: Everything is a Tool). The LLM chooses between Spawn and Delegate automatically; Team mode requires explicit user invocation.

**Runtime topology**: Named agents (Delegate, Team) dispatch through the
Orchestrator → AgentHarness pipeline introduced in Phase 5/6. The Spawn mode
(SubagentTool) was migrated to Harness in Phase 7 (2026-04-21). The legacy
`src/agent_loop/` directory has been deleted. All agent execution now routes
through Orchestrator → AgentHarness.

## Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│          User-Triggerable Collaboration Modes         │
│                                                        │
│  ┌───────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │   Spawn   │   │   Delegate   │   │     Team     │ │
│  │ subagent  │   │ session_send │   │  /team, /task │ │
│  │           │   │              │   │              │ │
│  │ Ephemeral │   │ Peer-to-peer │   │ Three-Layer  │ │
│  │ worker    │   │ messaging    │   │ Coordination │ │
│  └───────────┘   └──────────────┘   └──────────────┘ │
├──────────────────────────────────────────────────────┤
│        Swarm Sensing (automatic infrastructure)       │
│     Event Bus · Context Injector · Collective Memory  │
└──────────────────────────────────────────────────────┘
```

## Mode Comparison

| | Spawn | Delegate | Team |
|---|---|---|---|
| **Trigger** | LLM automatic | LLM automatic | User `/team` command |
| **Tools** | `subagent_spawn/steer/kill` | `session_send` | 9 team tools (see below) |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) |
| **Communication** | Return value | Messages (fire-and-forget or wait) | Three-layer: Tasks + Messages + Sessions |

## Mode 1: Spawn (Sub-Agent Dispatch)

**Tools**: `subagent_spawn`, `subagent_steer`, `subagent_kill`

The main agent spawns an ephemeral sub-agent to handle a focused sub-task. The
sub-agent has its own tool registry (excluding subagent tools to prevent
recursion), token budget, and timeout. It returns a result and is destroyed.

**Runtime**: Sub-agent spawning routes through Orchestrator → AgentHarness
(Phase 7, 2026-04-21). The legacy `src/agent_loop/` directory has been deleted.

**When LLM uses it**: Tasks benefiting from isolated context — parallel searches, code analysis, format conversion, translation.

**Swarm integration**: Sub-agent events are NOT published to the Event Bus (ephemeral, not a named agent).

### HarnessDeps inheritance (Stage 5a / Stage A, 2026-05-08)

Subagents inherit the following from their parent via `SpawnerBase`:

- `guardrails` (Stage 5a) — Input/Output/ToolCall checks
- `fallback_llm` (Stage A, 2026-05-08) — Stage 5b single-step fallback
- `stall_config`, `consecutive_failure_cap`, `turn_timeout` (Stage A) — P0 stability triple
- `trace_sink` (Stage A) — observability sink

Per the P1 zero-override decision, subagents do not currently support per-agent overrides for these fields. `AgentDef` may be extended with `Option<T>` overrides in P4 if needed, with full backward compatibility.

The shared assembly path lives in `src/orchestrator/deps_builder.rs` (`build_fallback_llm`, `build_stability_triple`); both the main runner (`aleph-server` boot) and the subagent spawner consume the same builders so wiring stays consistent.

### Recursion Protection

SubAgent-mode agents are structurally denied from invoking the `subagent`
tool. Enforcement lives in `AgentDef::is_tool_allowed`
(`src/agents/types.rs`), which overrides any explicit allowlist entry
(including wildcard `"*"`). Primary-mode agents retain full subagent
spawning capability.

One additional defense layer exists:
- `ChainContext::child()` depth guard (`subagent_spawner.rs:114-117`)
  returns `None` when `max_depth` is reached, surfacing as a `"chain
  depth exceeded"` error.

### Filesystem Agent Loading (P2 Stage E)

Aleph loads agent definitions from three tiers (highest precedence first):

1. **Project tier** — `<project>/.aleph/agents/*.md`
2. **User tier** — `~/.aleph/data/agents/*.md`
3. **Builtin tier** — hardcoded in `crate::agents::registry::builtin_agents()`

Higher tiers shadow lower tiers silently when an `id` collision occurs.
Shadow events are logged at `tracing::info!` level during startup
(no global `trace_sink` is available at init time — sinks are per-session
on `HarnessDeps`). The `id`, `winner_source`, and `shadowed_source` appear
as structured fields on the log record.

#### User-Authored Markdown Schema

User and project agents declare configuration in YAML frontmatter:

```yaml
---
id: my-research-agent           # required, must match filename stem
description: Researches topics  # required
when_to_use: When ...           # required
model_hint: claude-sonnet-4-6   # optional
allowed_tools: [glob, grep]     # optional
allowed_tool_sets: [INVESTIGATION]  # optional, see "Named Tool Sets"
denied_tools: []                # optional
max_iterations: 20              # optional
token_budget: 50000             # optional
context_mode: standalone        # optional
---

System prompt body...
```

#### System-Forced Fields

| Field    | Forced to                                  |
|----------|--------------------------------------------|
| `mode`   | `SubAgent` (writing `Primary` → schema error) |
| `source` | `User` or `Project` (auto, based on tier)  |

#### Failure Modes

- Malformed frontmatter / YAML parse error → file skipped, `tracing::warn` emitted
- Missing required field → file skipped, `tracing::warn` emitted
- File stem ≠ frontmatter `id` → file skipped, `tracing::warn` emitted
- `mode: Primary` declared → file skipped, `tracing::warn` emitted

Aleph-server continues startup with successfully-loaded agents only;
one bad file does not abort startup.

#### Reload

Filesystem agents are loaded once at startup. Modifying a markdown file
requires restarting `aleph-server`.

## Mode 2: Delegate (Peer Communication)

**Tool**: `session_send`

A named agent sends a message to another named agent. Two semantics:
- **Fire-and-forget** (`timeout_seconds=0`): Notification — "FYI, I found this"
- **Wait-for-reply** (`timeout_seconds>0`): Delegation — "Please analyze this and reply"

**When LLM uses it**: When another agent has relevant expertise, when work should be transferred to a specialist, when coordination is needed between peers.

**Swarm integration**: Both agents are named — their activities are visible to all agents via Event Bus. Communication governed by `AgentToAgentPolicy`.

## Mode 3: Team (Structured Coordination)

**Entry**: `/team` or `/task` slash commands

A user explicitly creates a team with a Leader, members with defined roles, and a task DAG with dependencies. The Leader coordinates work via prompt-driven intelligence (R8/R10).

Team mode uses a **three-layer communication model** for structured coordination:

```
Layer 3: CollaborativeSession ──── Synchronous multi-turn dialogue, shared context
         (sessions/)               Triggered by: explicit request OR leader-approved escalation
              ^ escalation suggestion
Layer 2: MessageRouter ──────────── SQLite inbox, to/cc routing, threads
         (messages/)                Async lightweight messages between any agents
              ^ auto-notifications on task events
Layer 1: TaskCoordinator ────────── CoordTask DAG (unified) + TaskArtifact system
         (artifacts.rs + swarm)     + lifecycle events
```

### Layer 1: TaskCoordinator

The task layer uses the unified `CoordTask` DAG system (from `agents/swarm/tasks/`) with DAG dependencies, priority, and metadata. `TeamTask` was retired in favor of this unified system.

**Task Artifact System**: Each task can have structured outputs that other agents reference.

```rust
pub struct TaskArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,  // Report, Code, Review, Discovery, Challenge, Custom(String)
    pub title: String,
    pub content: String,              // markdown
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
```

Storage: `task_artifacts` table in SQLite.

**Task Lifecycle Events**: Task state changes auto-generate Layer 2 messages (`MessageType::SystemNotification`):

| Event | to | cc |
|-------|----|----|
| Task created | assignee | leader |
| Task completed | leader | agents depending on this task |
| Task failed | leader | assignee's collaborators |
| Task rejected | assignee | critic |

**Tools**:
- `task_submit` — agent submits structured output for a task
- `task_read_artifact` — any agent reads artifact by task_id

### Layer 2: MessageRouter

SQLite-backed asynchronous messaging with to/cc routing, threading, and TTL expiration.

**Message Model**:

```rust
pub struct TeamMessage {
    pub id: String,
    pub team_id: String,
    pub from_agent: String,
    pub msg_type: MessageType,
    pub subject: String,
    pub content: String,
    pub recipients: Vec<Recipient>,    // to/cc list
    pub reply_to: Option<String>,      // threading
    pub thread_id: Option<String>,     // thread grouping
    pub attachments: Vec<String>,      // referenced artifact IDs
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}
```

**Message Types**:
- Business: `Message`, `Discovery`, `Challenge`, `ReviewRequest`, `ReviewResult`
- System: `SystemNotification` (auto-generated from Layer 1 task events)
- Lifecycle: `Idle`, `PlanApprovalRequest`, `PlanApproved`, `PlanRejected`

**Delivery semantics**:
- **to**: appears in inbox as actionable. Context injector prompts agent.
- **cc**: appears in inbox as informational. Lower priority, truncated first under context pressure.

**Consumption model**: Agent pulls via `inbox_read` tool. Context injector shows summary only ("You have N unread messages, M require action"). Agent decides when to read details (R8).

**Threading**: Messages grouped by `thread_id`. First message auto-creates thread (`thread_id = message_id`). Replies inherit `thread_id`. `inbox_read` with `mode: "thread"` reads full thread context.

**Message Expiration**: Default TTL: to messages 2h, cc messages 30m, SystemNotification 15m (configurable). Expired messages removed from inbox view, retained in event log.

**Tools**:
- `message_send` — send message with to/cc routing, reply_to for threading
- `inbox_read` — read inbox (filter by msg_type, unread) or read full thread (`mode: inbox/thread`)
- `team_digest` — generate team summary from event log (LLM-generated)

### Layer 3: CollaborativeSession

Synchronous multi-turn dialogue for situations requiring focused, real-time discussion between participants.

**Session Model**:

```rust
pub struct CollaborativeSession {
    pub id: String,
    pub team_id: String,
    pub participants: Vec<String>,
    pub topic: String,
    pub trigger: SessionTrigger,       // Explicit or AutoEscalation
    pub thread_id: Option<String>,     // inherited from L2 thread if escalated
    pub max_rounds: u32,
    pub status: SessionStatus,         // Active, Concluded, Deadlocked, Cancelled
    pub transcript: Vec<SessionTurn>,
    pub outcome: Option<SessionOutcome>,
    pub created_at: DateTime<Utc>,
}
```

**Execution**: The leader agent orchestrates collaborative sessions via tools — there is no code-level orchestrator. `CollaborativeSession` is a data structure, not an active process. The leader creates the session, participants exchange turns, and the leader finalizes the outcome. Round counting (`max_rounds`) is a tool-level guardrail: `session_turn` rejects submissions beyond max_rounds.

**Escalation**: Suggestion-based, not automatic. `EscalationRule` defines thresholds (e.g., thread message count > 5, review reject count > 3). When triggered, the `MessageRouter` sends a `SystemNotification` to the leader suggesting escalation. The leader (LLM) decides whether to actually escalate based on content, not just counts (R8).

**Tools**:
- `session_collaborate` — start collaborative session (participants, topic, max_rounds)
- `session_turn` — speak in session (`mode: respond` or `mode: conclude` to propose ending with outcome)
- `session_read` — read transcript and outcome

### Role Mechanism

Roles are implemented through **prompt templates + structured tools + validation rules**, not code-level behavior logic (R8, R10).

**AgentRole enum**: `Leader`, `Explorer`, `Critic`, `Worker`, `Custom(String)`

**Role-specific behavior** is defined entirely in prompt templates:

- **Explorer** — forced divergent thinking: generate multiple hypotheses, cite external sources, present counter-arguments to team consensus
- **Critic** — anti-agreeableness bias: no agreement openers, minimum challenge count, evidence-backed challenges, pass threshold enforcement

**ReviewScore** (`review_score` tool):

```rust
pub struct ReviewScore {
    pub task_id: String,
    pub artifact_id: String,
    pub scores: Vec<DimensionScore>,   // dimension + score (1-10) + rationale
    pub overall_pass: bool,
    pub challenges: Vec<Challenge>,     // point + severity (Critical/Major/Minor) + evidence
    pub improvement_suggestions: Vec<String>,
    pub risks_if_accepted: Vec<String>,
}
```

**Configurable validation** via `TeamRoleConfig`:
- `min_challenges` (default: 3) — `review_score` rejects submission if `challenges.len() < min_challenges`
- `min_score_threshold` (default: 7) — `overall_pass = true` only allowed when all scores >= threshold
- `review_dimensions` — configurable per role (default: correctness, completeness, clarity)
- Leader can override thresholds per task or per review round

**Explorer-Critic Interaction Flow** (defined in leader's orchestration prompt, not code):

```
1. Leader assigns task to Explorer
2. Explorer submits Discovery (via task_submit)
3. Auto-notify Critic (L1 event → L2 message)
4. Critic submits ReviewScore (via review_score)
5. If overall_pass = false:
   - Challenge sent to Explorer (L2 message)
   - Explorer revises and resubmits
   - Back to step 4
6. If back-and-forth exceeds threshold → escalate to L3 CollaborativeSession
7. Session outcome = final deliverable
```

### Context Integration

When the autonomous dispatcher launches a member agent for a task, it
assembles a deterministic **handoff context** (`teams::dispatcher::handoff`)
and passes it as the agent's input. Sections:

- **Task** — the task subject and description
- **Dependency Results** — the result of every completed upstream task (the
  DAG fan-in channel)
- **Team** — the member's identity and the team roster with roles
- **Inbox** — an unread-message summary (`InboxContext`); the agent reads the
  detail on demand via `inbox_read` (R8)

Every section is individually byte-capped so a large DAG cannot blow the
prompt. There is no background "sensing" loop — context is gathered once, at
launch, from the task store and team state.

### Event Log

All operations logged as `TeamEvent` with retention policy:

```rust
pub enum TeamEventType {
    MessageSent,
    MessageRead,
    TaskCreated,
    TaskCompleted,
    TaskFailed,
    ArtifactSubmitted,
    ReviewScoreSubmitted,
    SessionStarted,
    SessionConcluded,
    DigestGenerated,
}
```

**Retention**: Active teams — events older than 72h pruned lazily (during `team_digest` generation or periodic cleanup). Disbanded teams — events older than 24h pruned. Used for `team_digest` generation.

## Infrastructure: Autonomous Dispatcher

The **`TeamDispatcher`** (`src/teams/dispatcher/`) drives a team's
coordination-task DAG to completion without leader micro-management.

- **Event-driven**: `task_create` and task completion signal the dispatcher
  (a `tokio::sync::Notify`); there is no polling. A fallback tick (default
  60 s) catches any missed signal.
- **In-process**: members run as `tokio` tasks via the shared execution path
  (`execute_member_task`) — real cancellation, no process-spawn cost, no
  scheduling latency.
- **DAG as protocol**: a task created with `blocked_by` edges runs only once
  every dependency is `Completed` (`CoordTaskStore` derives the `Blocked`
  status dynamically). Fan-in is simply a task that depends on several others.
- **Claiming**: each runnable task is claimed with an atomic lock; a
  concurrency cap (`DispatcherConfig::max_concurrent`, default 4) bounds
  parallelism.
- **Restart reconciliation**: on startup, in-progress tasks left by a previous
  process are reclaimed and rescheduled.
- **Unknown owner** is an explicit failure — the task is marked `Failed` with a
  clear error rather than left silently stuck.

Only tasks created via `task_create` (tagged `managed_by: dispatcher` in
metadata) are autonomous. `team_delegate` runs a single member synchronously
and is never picked up by the dispatcher.

Task outcomes are broadcast as `TeamTaskCompleted` / `TeamTaskFailed` events;
the **`TeamNotifier`** routes them to the team leader's inbox (R5).

## Mode Selection Guide

| Need | Mode |
|------|------|
| Simple sub-task (search, compute, format) | Spawn |
| Ask another agent a question | Delegate |
| Notify another agent of a finding | Delegate (fire-and-forget) |
| Hand off a task to a specialist | Delegate (wait-for-reply) |
| Multi-step project with dependencies | Team |
| Code review with multiple reviewers | Team |
| Multi-analyst decision making | Team |
| Explorer-Critic research cycle | Team (with roles) |

## Tool Reference

### Spawn
| Tool | Description |
|------|-------------|
| `subagent_spawn` | Spawn an ephemeral sub-agent for a focused task |
| `subagent_steer` | Send guidance to a running sub-agent |
| `subagent_kill` | Terminate a running sub-agent |

### Delegate
| Tool | Description |
|------|-------------|
| `session_send` | Send message to another agent's session (fire-and-forget or wait) |

### Team — Layer 1 (Tasks & Coordination)
| Tool | Description |
|------|-------------|
| `team_create` | Create a team with a leader and members |
| `team_delegate` | Delegate one task to a member synchronously and wait for the reply |
| `team_status` | Inspect a team's members and tasks |
| `team_disband` | Disband a team |
| `team_member_remove` | Remove a member from a team |
| `task_create` | Create a coordination task (with `blocked_by` deps) for autonomous dispatch |
| `task_update` | Update a task's status / result |
| `task_list` | List coordination tasks |
| `task_wait` | Wait for a task to reach a terminal state |
| `task_submit` | Submit a structured artifact for a task |
| `task_read_artifact` | Read artifact(s) by task ID |

### Team — Layer 2 (Messages)
| Tool | Description |
|------|-------------|
| `message_send` | Send message with to/cc routing, threading, attachments |
| `inbox_read` | Read inbox (filter by type/unread) or full thread (mode: inbox/thread) |
| `team_digest` | Generate LLM-summarized team digest from event log |

### Team — Layer 3 (Collaborative Sessions)
| Tool | Description |
|------|-------------|
| `session_collaborate` | Start a collaborative session (participants, topic, max_rounds) |
| `session_turn` | Speak in session (mode: respond or conclude) |
| `session_read` | Read session transcript and outcome |

### Team — Plan Approval
| Tool | Description |
|------|-------------|
| `plan_submit` | Submit a plan for team-leader approval before starting significant work |
| `plan_resolve` | Approve or reject a submitted plan (team leader) |

### Team — Roles
| Tool | Description |
|------|-------------|
| `review_score` | Submit structured review with configurable validation (Critic tool) |

## Module Structure

```
src/teams/
├── mod.rs                  // re-exports
├── types.rs                // Team, TeamMember, TeamStatus, TeamSummary
├── store.rs                // SqliteTeamStore (team/member management)
├── artifacts.rs            // TaskArtifact, ArtifactType, storage
├── events.rs               // TeamEvent, TeamEventType, TeamEventLogger
├── context.rs              // InboxContext, InboxContextProvider
├── plans.rs                // PlanManager (plan submit / approve / reject)
├── notifier.rs             // TeamNotifier (task outcomes → leader inbox)
├── dispatcher/
│   ├── mod.rs              // TeamDispatcher, DispatcherConfig, dispatch loop
│   ├── schedule.rs         // dispatch_once, select_schedulable, run_task
│   ├── runner.rs           // execute_member_task (shared with team_delegate)
│   └── handoff.rs          // build_handoff_context
├── messages/
│   ├── mod.rs
│   ├── types.rs            // TeamMessage, Recipient, MessageType
│   ├── router.rs           // MessageRouter: send, broadcast, route
│   ├── inbox.rs            // Inbox: read, peek, thread, expire
│   └── store.rs            // SQLite message storage
└── sessions/
    ├── mod.rs
    ├── types.rs            // CollaborativeSession, SessionTurn, SessionOutcome
    ├── store.rs            // SQLite session storage
    └── coordinator.rs      // SessionCoordinator

The coordination-task DAG store (CoordTaskStore / coord_tasks) lives in
src/agents/swarm/tasks/.
```

## Design Principles

These modes are **conceptual categories, not runtime states**. There is no mode-switching code. The LLM selects tools based on their descriptions (R8: LLM Sovereignty). The system provides the primitives; the LLM provides the intelligence.

Key design decisions for Team mode:
- **Task system**: Unified on `CoordTask` (retired `TeamTask`) — DAG dependencies, priority, metadata
- **Storage**: SQLite (ACID, queryable, consistent with rest of Aleph)
- **Routing**: to + cc (no bcc) — simplicity; agents don't need hidden information flows
- **Message status**: Derived from `read_at` + `expires_at` (not stored separately) — avoids sync issues
- **Role behavior**: Prompt-based, not code (R8/R10)
- **Escalation**: Suggestion to leader, not auto-action (R8)
- **Session orchestration**: Leader agent via tools, not code-level orchestrator (R8)
- **Context management**: Tools return full content; agent loop handles truncation
- **Critic validation**: Configurable thresholds from `TeamRoleConfig` — tool constraint (empowerment), not reasoning replacement
- **Tool count**: 9 new tools (consolidated `inbox_read` + thread, `session_turn` + conclude) — fewer tools improve LLM tool selection accuracy

See also:
- [AGENT_SYSTEM.md](AGENT_SYSTEM.md) — Single-agent internals (loop, guards, state machine)
- [TOOL_SYSTEM.md](TOOL_SYSTEM.md) — Tool registration and execution

## Subagent Progress Streaming (P2 Stage F)

Background subagents emit a structured progress trail observable to the parent
through the `subagent` tool's `check_status` action. Sync (foreground) subagents
do NOT participate in this mechanism — their final result is the only signal.

### Progress Event Schema

```rust
struct SubagentProgress {
    step: usize,                    // child harness iteration
    timestamp: SystemTime,          // wall-clock at translation
    kind: ProgressKind,             // ToolCalled | ToolReturned | LlmThinking | Cancelled
    tool_name: Option<String>,      // Some for ToolCalled / ToolReturned
    latency_ms: Option<u64>,        // Some for ToolReturned (call duration)
    preview: Option<String>,        // Some for ToolReturned (200-char truncation)
}
```

### Wiring (R10-Safe Decorator)

`ForwardingTraceSink` (in `src/agents/forwarding_trace_sink.rs`) wraps the
parent-inherited `trace_sink` exclusively for background subagents. It:

1. Translates `LoopTraceEvent::ToolCallStarted` / `ToolCallCompleted` /
   `TurnStateEntered{Think}` / `SessionCompleted{Cancelled}` into
   `SubagentProgress`
2. Pushes the translated event onto `BackgroundAgentTracker.progress` (FIFO,
   capped at 50)
3. Always forwards the original event to the inner sink (preserves
   gateway/disk trace flow)

Other LoopTraceEvent variants pass through untranslated. Adding new translation
cases does not require harness changes.

### check_status Output Shape

When status == "running", the response includes a `progress` field:

```json
{
  "status": "running",
  "request_id": "...",
  "progress": [
    { "step": 0, "kind": "llm_thinking", ... },
    { "step": 1, "kind": "tool_called", "tool_name": "grep", ... },
    { "step": 1, "kind": "tool_returned", "latency_ms": 42, "preview": "...", ... }
  ]
}
```

Up to 10 most-recent events are returned. The buffer caps at 50 internally;
older events are evicted FIFO.

### Why cap=50?

This is a designed memory/observability tradeoff (P2 Q6, hardcoded). For
long-running background subagents (>50 tool calls), only the most recent 50
steps remain visible. Configurable cap is a future stage if needed.

## Named Tool Sets (P2 Stage G)

`AgentDef.allowed_tool_sets: Vec<String>` lets agent definitions reference named
tool collections instead of (or alongside) flat allowlists. Three sets are
predefined:

| Name           | Tools                                                  | Purpose                                       |
|----------------|--------------------------------------------------------|-----------------------------------------------|
| `READ_ONLY`    | glob, grep, read_file                                  | Pure filesystem inspection                    |
| `INVESTIGATION`| glob, grep, read_file, search, web_fetch, subagent     | Read-only research with remote sources        |
| `ASYNC_SAFE`   | glob, grep, read_file, search                          | Background-safe (no side effects, no exfil)   |

### Composition Rules

`AgentDef::is_tool_allowed(tool)` evaluates in this precedence:

1. **Recursion guard** (Stage B): SubAgent mode → `subagent` tool denied
   regardless of allowlist
2. **Explicit deny**: tool in `denied_tools` → denied
3. **Set match**: tool in any resolved `allowed_tool_sets` member → allowed
4. **Flat match**: tool in `allowed_tools` (with `"*"` wildcard) → allowed
5. **Default**: denied

`denied_tools` always wins over set membership; this lets agents use a broad
named set then selectively exclude.

### Example

```yaml
---
id: my-research-agent
allowed_tool_sets: [INVESTIGATION]
denied_tools: [web_fetch]   # narrow the broad set
---
```

Effective allowed: glob, grep, read_file, search (web_fetch denied; subagent
denied via mode guard since this is a SubAgent).

### Unknown Set Names

`resolve` returns `None` for unknown names; the loader emits a warning
but doesn't fail. This allows future named sets to be added without breaking
older agent definitions.

### Builtin Agents Using Named Sets

| Agent     | Migration                                  |
|-----------|--------------------------------------------|
| `explore` | `INVESTIGATION` (P2 Stage G demo)          |

Other builtins still use flat `allowed_tools`; migrations are incremental
and require behavior-equivalence verification (see `tests/tool_sets.rs`).

## Subagent Concurrency Cap

Every `SubagentTool` instance owns a `tokio::sync::Semaphore` (default 4
permits) shared across the foreground, sync-batch, and background spawn
paths. `subagent_spawner::spawn` acquires a permit before running the child
harness and holds it for the child's lifetime, so a single `subagent` call
with a large `batch_tasks` array queues on the semaphore instead of fanning
out unbounded `tokio` tasks. `SpawnerBase.subagent_semaphore` is `None` for
direct test callers (no cap). This is the lightweight replacement for the
retired lane scheduler.

## Worktree Isolation (P3 Stage H)

Subagent spawns can opt into git worktree isolation:

```rust
let req = SpawnRequest {
    agent_def: &agent_def,
    task: "refactor module X",
    context_summary: None,
    model: None,
    timeout_secs: 600,
    cancel: cancel_token,
    isolation: Some(IsolationMode::Worktree), // P3 Stage H
};
```

When set to `Worktree`, the spawner creates a fresh detached-HEAD git
worktree at `$TMPDIR/aleph-subagent-<safe_label>-<uuid>/` before running
the child harness. The child's `Sandbox` is replaced with `WorktreeSandbox`,
which executes commands at the worktree path and injects `CARGO_TARGET_DIR`
for strict build isolation.

A subagent dispatched through the production gateway path (`AgentRuntime`)
sources `isolation` from its `AgentDef`: declare `isolation:` with
`kind: worktree` in an agent's markdown frontmatter to opt that named agent
into worktree isolation. Builtin agents leave it unset (shared cwd) —
worktree isolation changes where a child's file edits land, so it is opt-in
per agent definition.

Cleanup is RAII-guaranteed:
- **Success path**: explicit `cleanup().await` after harness returns.
- **Error/timeout/panic path**: `Drop` safety-net spawns a blocking
  `git worktree remove --force` and emits
  `LoopTraceEvent::WorktreeCleanedUp { leaked: true }`.

### Scope

`WorktreeSandbox` provides **workspace isolation only** — it does not apply
seatbelt or capability baseline. For seatbelt-protected subagents, omit
`isolation` (or set to `None`) and trust the parent's `WorkspaceSandbox`.
This is a deliberate Stage H scope choice; combining seatbelt + worktree
is a follow-up.

### Trace events

`LoopTraceEvent::WorktreeCreated { path }` and
`LoopTraceEvent::WorktreeCleanedUp { path, leaked: bool }` flow into
the parent's `trace_sink`. Use `leaked` to distinguish explicit cleanup
from Drop safety-net cleanup in monitoring dashboards.

### Performance contract

- `create`: ≤ 200ms typical (`git worktree add` cost)
- `cleanup`: ≤ 100ms typical (`git worktree remove --force` cost)

### Failure mode

Worktree creation failure is **fail-loud**: spawner returns
`"sub-agent failed: worktree create: ..."`. There is no fallback to shared
cwd — isolation declared must be isolation honored.

## Per-Agent MCP Scope (P3 Stage I)

Subagents can declare which MCP servers they need:

```yaml
---
id: git-research
description: explores the local git repo
when_to_use: when investigating commit history
mcp_servers:
  - type: reference
    name: github
  - type: inline
    name: local-git-mcp
    config:
      command: /usr/local/bin/local-git-mcp
      args: ["--readonly"]
      env:
        GIT_PAGER: cat
---
```

`Reference` reuses a server already registered in the global `McpRegistry`.
`Inline` spawns a fresh process owned by **only this subagent's lifetime** —
not shared across agents, not warm-pooled.

### Provisioning model

When `mcp_servers` is non-empty, the spawner runs `McpScope::provision`
**before** building `HarnessDeps`:

1. Phase 1 — classify specs; validate `Inline { name }` does not collide
   with a name already in the global registry (`McpScopeError::NameConflict`).
2. Phase 2 — spawn all inline servers eagerly + in parallel via
   `try_join_all`. Performance soft contract: ≤ 500ms.

The scope's tools are layered **under** `AllowlistToolService`, so the
recursion guard (Stage B) and per-agent denylist still apply on top.

### Cleanup

- **Success path**: explicit `scope.shutdown().await` after harness returns Ok.
- **Error/timeout/panic**: `Drop` safety-net emits
  `LoopTraceEvent::McpScopeCleaned { leaked: true }` and triggers
  `InlineMcpHandle::Drop` for each inline process (sync OS thread → kill).

### Trace events

- `LoopTraceEvent::McpScopeAttached { agent_id, references, inline_count }`
- `LoopTraceEvent::McpScopeCleaned { agent_id, leaked }`

Both bridge to `aleph_protocol::AgentTraceEvent` with the same field shape.

### Failure modes

| Path | Mapping |
|---|---|
| Inline name vs global collision | `McpScopeError::NameConflict` → `"sub-agent failed: mcp scope: ..."` |
| Reference name not in global | `McpScopeError::ReferenceNotFound` → same |
| Inline process startup failure | `McpScopeError::InlineStartup` → same |
| Inline process shutdown failure | `McpScopeError::InlineShutdown` → logged via `tracing::error`, harness Ok preserved |

There is no fallback to "global-only" tools when scope provisioning fails —
declared scope is honored or the spawn fails loudly (per design § 3 Q8).

### Out of scope (P3 Stage I)

- Inter-agent inline server sharing
- Warm-pool / pre-spawn
- Per-tool execution budgets
- Health monitoring / heartbeat / restart
- Seatbelt enforcement on inline processes
- Inline-server tool surfacing (deferred — Task 8 known concern: `tools()` only includes Reference-projected globals at this stage; `McpServerConnection::list_tools()` is async and requires a snapshot-at-provision-time mechanism that is a follow-up)

These are deferred per design § 5.

## Cache Observability Pipeline (Stage J-pre)

The `MeteringProvider` decorator (`src/providers/metering.rs`) wraps every
LLM-facing `Arc<dyn AiProvider>` and emits a `LoopTraceEvent::ProviderUsage`
event after each `process()` call. The event carries:

- `agent_id` — `"root"` for the top-level harness, or the subagent's
  `agent_def.id` when emitted from within a spawned subagent
- `input_tokens` / `output_tokens` — total tokens charged
- `cache_read_tokens` / `cache_creation_tokens` — Anthropic prompt-cache
  fields (other providers leave these `None` until they extend their
  protocols)
- `thinking_tokens` — Gemini extended-thinking tokens (where applicable)

The decorator is wrapped at exactly two sites:

- `src/bin/aleph-server/commands/start/orchestrator_init.rs` — root
  provider, label `"root"`
- `src/agents/subagent_spawner.rs` — per-spawn, label `req.agent_def.id`

This gives every consumer of the trace stream (gateway, log sink, future
cost dashboard) the data needed to compute root vs subagent cache-hit
ratios. Stage J's "fork branch" decision is gated on collecting ≥2 weeks
of this data starting from the J-pre ship date — see roadmap § 1.2 Stage J.

R10 redline preserved: the decorator does not touch `src/harness/agent.rs`;
the `LoopTraceEvent::ProviderUsage` variant is schema-only (mirrors into
`AgentTraceEvent`). The harness loop remains unchanged.
