# Multi-Agent System

Aleph provides three user-triggerable collaboration modes plus a shared sensing infrastructure layer. All modes operate through tools (R9: Everything is a Tool). The LLM chooses between Spawn and Delegate automatically; Team mode requires explicit user invocation.

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

The main agent spawns a temporary AgentLoop to handle a focused sub-task. The sub-agent has its own tool registry (excluding subagent tools to prevent recursion), token budget, and timeout. It returns a result and is destroyed.

**When LLM uses it**: Tasks benefiting from isolated context — parallel searches, code analysis, format conversion, translation.

**Swarm integration**: Sub-agent events are NOT published to the Event Bus (ephemeral, not a named agent).

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

**InboxContext** injected into the agent loop via the existing `ContextInjector`:

```rust
pub struct InboxContext {
    pub unread_to: u32,
    pub unread_cc: u32,
    pub urgent_summary: String,
}
```

**Injection rules**:
- **to messages**: summary always injected (high priority)
- **cc messages**: count only ("3 cc messages"), no content
- **Collaborative session invite**: strong prompt injection
- Agent reads details via `inbox_read` on demand (R8)

**Context priority** (high to low):
1. Current task instructions
2. to messages (actionable)
3. Tool results / work output
4. cc messages (informational)
5. Team digest (background)
6. ← truncation line adjusts dynamically based on token budget →

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

## Infrastructure: Swarm Sensing

**Components**: Event Bus, Rules Engine, Semantic Aggregator, Context Injector, Collective Memory

Not a user-triggerable mode. Automatically active for all named agents. Provides:
- Cross-agent activity awareness (who is doing what)
- Hotspot detection (multiple agents working in same area)
- Contextual injection (team task boards, swarm activity summaries)
- Collective memory for shared knowledge

All three modes benefit from swarm sensing — it is the shared perception layer that makes multi-agent collaboration intelligent.

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

### Team — Layer 1 (Tasks & Artifacts)
| Tool | Description |
|------|-------------|
| `team_create` | Create a team with a leader |
| `team_list` | List all teams and their status |
| `team_disband` | Disband a team, cancelling remaining tasks |
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

### Team — Roles
| Tool | Description |
|------|-------------|
| `review_score` | Submit structured review with configurable validation (Critic tool) |

## Module Structure

```
src/teams/
├── mod.rs                  // re-exports
├── types.rs                // Team, TeamMember, TeamStatus
├── store.rs                // SqliteTeamStore (team/member management)
├── artifacts.rs            // TaskArtifact, ArtifactType, storage
├── events.rs               // TeamEvent, TeamEventType, event log store
├── context.rs              // InboxContext, context injection
├── messages/
│   ├── mod.rs
│   ├── types.rs            // TeamMessage, Recipient, MessageType
│   ├── router.rs           // MessageRouter: send, broadcast, route
│   ├── inbox.rs            // Inbox: read, peek, thread, expire
│   └── store.rs            // SQLite message storage
├── sessions/
│   ├── mod.rs
│   ├── types.rs            // CollaborativeSession, SessionTurn, SessionOutcome
│   ├── store.rs            // SQLite session storage
│   └── coordinator.rs      // SessionCoordinator, EscalationRule
└── roles/
    ├── mod.rs
    ├── types.rs            // AgentRole, TeamRoleConfig, Severity
    └── review.rs           // ReviewScore, DimensionScore, Challenge, validation
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
