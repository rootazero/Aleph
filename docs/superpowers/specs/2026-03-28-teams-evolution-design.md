# Teams Evolution: Three-Layer Communication + Role Mechanism

**Date**: 2026-03-28
**Status**: Draft
**Reference**: ClawTeam (`~/Github/ClawTeam`) as conceptual model

## Overview

Redesign Aleph's teams module from a leader-centric task delegation system into a three-layer communication architecture with structured role mechanisms (Explorer/Critic). The approach reuses existing infrastructure (SQLite, event bus, swarm coordinator, CoordTask DAG) while introducing a new MessageRouter layer and role-driven interaction patterns.

### Design Principles

- **R8 compliance**: System provides communication tools; LLM decides when/how to use them
- **R10 compliance**: Role behavior lives in prompt templates, not code logic
- **Incremental delivery**: Layer 1 (enhance) -> Layer 2 (new) -> Layer 3 (new) -> Roles (prompt-based)
- **Borrow concepts, not code**: ClawTeam's inbox/event-log separation and lifecycle messages inform the design, but implementation uses SQLite (not file-based inbox) since Aleph is a single-host application

## Task System Unification

**Decision**: Retire `TeamTask` (`teams/types.rs`) in favor of `CoordTask` (`agents/swarm/tasks/`).

- `CoordTask` is strictly superior: DAG dependencies, priority, metadata, blocked_by
- `TeamTask` is a simpler duplicate with no unique capabilities
- `team_delegate` migrates from `TeamStore::create_task` to `CoordTaskStore::create`
- All artifact references (`TaskArtifact.task_id`) point to `CoordTask.id`
- `TeamStore` retains team/member management only; task storage moves entirely to `CoordTaskStore`

## Architecture: Three-Layer Model

```
Layer 3: CollaborativeSession ──── Synchronous multi-turn dialogue, shared context
         (new module)              Triggered by: explicit request OR leader-approved escalation
              ^ escalation suggestion
Layer 2: MessageRouter ──────────── SQLite inbox, to/cc routing, threads
         (new module)               Async lightweight messages between any agents
              ^ auto-notifications on task events
Layer 1: TaskCoordinator ────────── CoordTask DAG (unified) + team_delegate
         (enhanced)                 + artifact system + lifecycle events
```

Communication topology:
- **Leader defines initial topology** at team creation (who talks to whom about what)
- **Agents can initiate additional communication** autonomously (R8 — LLM judges relevance)

---

## Layer 1: TaskCoordinator Enhancement

### 1.1 Task Artifact System

Each task can have structured outputs (artifacts) that other agents can reference:

```rust
pub struct TaskArtifact {
    pub task_id: String,
    pub agent_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: String,              // markdown
    pub metadata: serde_json::Value,  // scores, tags, structured data
    pub created_at: DateTime<Utc>,
}

pub enum ArtifactType {
    Report,      // research reports, analysis conclusions
    Code,        // code change summaries
    Review,      // review opinions (Critic output)
    Discovery,   // exploration findings (Explorer output)
    Challenge,   // challenge documents (Critic output)
    Custom(String), // extensible for new role types
}
```

**Storage**: `task_artifacts` table in existing SQLite store.

**Tools**:
- `task_submit` — agent submits structured output for a task
- `task_read_artifact` — any agent reads artifact by task_id

### 1.2 Task Lifecycle Events

Task state changes auto-generate Layer 2 messages (`MessageType::SystemNotification`):

| Event | to | cc |
|-------|----|----|
| Task created | assignee | leader |
| Task completed | leader | agents depending on this task |
| Task failed | leader | assignee's collaborators |
| Task rejected | assignee | critic |

### 1.3 Changes to Existing Code

- `CoordTaskStore`: add `task_artifacts` table + CRUD
- `team_delegate`: auto-call `task_submit` on completion to persist output
- New tools: `task_submit`, `task_read_artifact`

---

## Layer 2: MessageRouter (Core New Module)

### 2.1 Message Model

```rust
pub struct TeamMessage {
    pub id: String,                    // uuid
    pub team_id: String,
    pub from_agent: String,
    pub msg_type: MessageType,
    pub subject: String,               // short subject for scanning
    pub content: String,               // markdown body
    pub recipients: Vec<Recipient>,    // to/cc list
    pub reply_to: Option<String>,      // reply to which message (thread)
    pub thread_id: Option<String>,     // thread grouping
    pub attachments: Vec<String>,      // referenced artifact IDs
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,  // computed at send time from TTL rules
    // Note: message status (unread/read/expired) is derived from message_recipients.read_at
    // and expires_at, not stored as a separate field
}

pub struct Recipient {
    pub agent_id: String,
    pub role: RecipientRole,           // To, Cc
}

pub enum RecipientRole {
    To,   // must process/respond
    Cc,   // informational only
}

pub enum MessageType {
    // Business messages
    Message,              // general communication
    Discovery,            // Explorer findings
    Challenge,            // Critic challenges
    ReviewRequest,        // request for review
    ReviewResult,         // review result (with scores)

    // System messages
    SystemNotification,   // auto-generated from Layer 1 task events

    // Lifecycle (inspired by ClawTeam)
    Idle,                 // work done, awaiting new task
    PlanApprovalRequest,  // request leader to approve plan
    PlanApproved,
    PlanRejected,
}

// MessageStatus is derived, not stored:
// - Unread: message_recipients.read_at IS NULL AND expires_at > now
// - Read: message_recipients.read_at IS NOT NULL
// - Expired: expires_at <= now AND read_at IS NULL
```

### 2.2 Inbox Mechanism

SQLite-backed logical inbox per agent:

```sql
-- messages table
CREATE TABLE team_messages (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL,
    from_agent TEXT NOT NULL,
    msg_type TEXT NOT NULL,
    subject TEXT NOT NULL,
    content TEXT NOT NULL,
    reply_to TEXT,
    thread_id TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT  -- computed at send time: created_at + TTL based on recipient roles
);

-- Performance index for thread queries
CREATE INDEX IF NOT EXISTS idx_team_messages_thread ON team_messages(thread_id);
CREATE INDEX IF NOT EXISTS idx_team_messages_team ON team_messages(team_id, created_at);

-- recipients table (to/cc routing)
CREATE TABLE message_recipients (
    message_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,  -- 'to' or 'cc'
    read_at TEXT,
    PRIMARY KEY (message_id, agent_id)
);

-- attachments
CREATE TABLE message_attachments (
    message_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    PRIMARY KEY (message_id, artifact_id)
);
```

**Delivery semantics**:
- **to**: appears in inbox, marked as actionable. Context injector prompts agent.
- **cc**: appears in inbox, informational. Lower priority, truncated first under context pressure.

**Consumption model**:
- Agent pulls via `inbox_read` tool (not auto-injected per message)
- Context injector shows summary only: "You have N unread messages, M require action"
- Agent decides when to read (R8 — LLM judges priority)

### 2.3 Threading

Messages grouped by `thread_id`:
- First message auto-creates thread (thread_id = message_id)
- Replies inherit thread_id — **invariant**: all messages in a thread share the same thread_id, which equals the first message's id
- `inbox_thread` tool reads full thread context
- Explorer<->Critic discussions form complete threads; leader or anyone can trace back

### 2.4 Message Expiration

- Default TTL: to messages 2h, cc messages 30m, SystemNotification 15m (configurable)
- Expired messages removed from inbox view, retained in event log
- Prevents context pollution in long-running teams

### 2.5 Team Digest

```rust
pub struct TeamDigest {
    pub team_id: String,
    pub generated_by: String,
    pub period: (DateTime<Utc>, DateTime<Utc>),
    pub summary: String,             // LLM-generated summary
    pub key_decisions: Vec<String>,
    pub open_issues: Vec<String>,
}
```

- Any agent can generate via `team_digest` tool (LLM reads event log, compresses to summary)
- When leader generates: broadcast as `SystemNotification` to all members (cc)
- When non-leader generates: private digest for own understanding (no broadcast)
- Also triggered at key milestones (phase completion, direction change)
- `team_id` parameter required (agents may be members of multiple teams)

### 2.6 Tools

| Tool | Description |
|------|-------------|
| `message_send` | Send message (to/cc/content/reply_to) |
| `inbox_read` | Read inbox (filter by msg_type, unread) or read full thread (mode: inbox/thread, thread_id) |
| `team_digest` | Generate team summary |

Note: `inbox_read` with `mode: "thread"` + `thread_id` replaces a separate `inbox_thread` tool — fewer tools, better LLM tool selection accuracy.

---

## Layer 3: CollaborativeSession

### 3.1 Session Model

```rust
pub struct CollaborativeSession {
    pub id: String,
    pub team_id: String,
    pub participants: Vec<String>,
    pub topic: String,
    pub trigger: SessionTrigger,
    pub thread_id: Option<String>,     // inherited from L2 thread if escalated
    pub max_rounds: u32,
    pub status: SessionStatus,
    pub transcript: Vec<SessionTurn>,
    pub outcome: Option<SessionOutcome>,
    pub created_at: DateTime<Utc>,
}

pub enum SessionTrigger {
    Explicit { requested_by: String },
    AutoEscalation {
        thread_id: String,
        message_count: u32,
        rule: String,
    },
}

pub struct SessionTurn {
    pub agent_id: String,
    pub content: String,
    pub turn_number: u32,
    pub timestamp: DateTime<Utc>,
}

pub enum SessionStatus {
    Active,
    Concluded,
    Deadlocked,
    Cancelled,
}

pub struct SessionOutcome {
    pub conclusion: String,
    pub agreed_by: Vec<String>,
    pub dissent: Option<String>,
}
```

### 3.2 Execution Mechanism

**The leader agent orchestrates collaborative sessions via tools — there is no code-level orchestrator.**

`CollaborativeSession` is a data structure, not an active process. The leader:
1. Creates session via `session_collaborate` tool (creates record, notifies participants)
2. Participants exchange messages within shared `thread_id` using `session_respond`
3. Leader monitors progress (via inbox) and decides when to prompt convergence
4. Any participant proposes conclusion via `session_conclude`; others agree or dissent
5. Leader finalizes: `SessionOutcome` saved, transcript archived, conclusion becomes `TaskArtifact`

Round counting (`max_rounds`) is a tool-level guardrail: `session_respond` rejects submissions beyond max_rounds and notifies participants "final round — submit your conclusion." This is a tool constraint (like review_score validation), not LLM reasoning replacement.

### 3.3 Escalation Rules (Suggestion-Based)

```rust
pub struct EscalationRule {
    pub thread_message_threshold: u32,   // default: 5
    pub review_reject_threshold: u32,    // default: 3
    pub enabled: bool,
}
```

- Configured by leader at team creation (or defaults)
- MessageRouter checks after each message delivery
- **Triggered → sends SystemNotification to leader**: "Thread X between Agent-A and Agent-B has N messages — consider starting a collaborative session"
- **Leader decides** whether to actually escalate (R8 — LLM judges whether escalation is warranted based on content, not just count)
- This avoids false escalations on productive back-and-forth that happens to exceed the threshold

### 3.4 Tools

| Tool | Description |
|------|-------------|
| `session_collaborate` | Start collaborative session (participants, topic, max_rounds) |
| `session_turn` | Speak in session (mode: respond/conclude — conclude proposes ending with outcome) |
| `session_read` | Read transcript/outcome |

Note: `session_turn` merges respond + conclude into one tool with a `mode` parameter.

---

## Explorer / Critic Role Mechanism

### Design Philosophy

Roles are implemented through **prompt templates + structured tools + validation rules**, not code-level behavior logic (R8, R10).

### Explorer Role

**Prompt strategy** — force divergent thinking and external information injection:

```markdown
## Explorer Role Instructions

You are an explorer. Your job is to discover possibilities others haven't considered.

### Behavioral Constraints
1. **Diverge first**: Generate at least 3 different hypotheses before evaluating
2. **No premature convergence**: Forbidden words in exploration phase: "obviously",
   "without doubt", "the best approach is"
3. **External info duty**: Must call at least one external search tool before concluding
4. **Anti-consensus obligation**: If team has consensus, must present at least one
   counter-argument

### Output Format (Discovery)
- hypotheses: at least 3
- evidence: supporting/opposing evidence per hypothesis
- external_sources: referenced external information
- recommended_direction: with rationale
- contrarian_view: counter-argument
```

### Critic Role

**Prompt strategy** — resist LLM's agreeableness bias:

```markdown
## Critic Role Instructions

You are a critic. Your job is to find flaws, challenge assumptions, ensure quality.

### Behavioral Constraints
1. **No agreement openers**: Never start with "this is good", "I agree", "makes sense"
2. **Minimum 3 challenges**: Every review must raise at least 3 specific issues
3. **Evidence required**: Each challenge must explain "why this might be wrong"
4. **Pass threshold**: Only pass when ALL scoring dimensions >= 7/10
```

**Structured scoring tool** (`review_score`):

```rust
pub struct ReviewScore {
    pub task_id: String,
    pub artifact_id: String,
    pub scores: Vec<DimensionScore>,
    pub overall_pass: bool,
    pub challenges: Vec<Challenge>,
    pub improvement_suggestions: Vec<String>,
    pub risks_if_accepted: Vec<String>,
}

pub struct DimensionScore {
    pub dimension: String,    // credibility, evidence, logic, innovation, feasibility
    pub score: u8,            // 1-10
    pub rationale: String,
}

pub struct Challenge {
    pub point: String,
    pub severity: Severity,   // Critical, Major, Minor
    pub evidence: String,
}
```

**System-level enforcement** (code-level tool constraints, not reasoning replacement):
- `review_score` reads `min_challenges` and `min_score_threshold` from `TeamRoleConfig` for the Critic's role in this team
- Rejects submission if `challenges.len() < config.min_challenges`
- `overall_pass = true` only allowed when all `scores >= config.min_score_threshold`
- Leader can override per-task or per-review-round via `TeamRoleConfig` update
- Defaults: `min_challenges = 3`, `min_score_threshold = 7` — suitable for most tasks; leader should lower for trivial reviews or raise for security-critical ones

### Explorer <-> Critic Interaction Flow

Defined in **leader's orchestration prompt**, not in code:

```
1. Assign task to Explorer
2. Explorer submits Discovery (via task_submit)
3. Auto-notify Critic (L1 event -> L2 message)
4. Critic submits ReviewScore (via review_score)
5. If overall_pass = false:
   - Challenge sent to Explorer (L2 message)
   - Explorer revises and resubmits
   - Back to step 4
6. If back-and-forth exceeds threshold -> escalate to L3 CollaborativeSession
7. Session outcome = final deliverable
```

### Role Configuration

```rust
pub struct TeamRoleConfig {
    pub role: AgentRole,
    pub prompt_template: String,
    pub review_dimensions: Vec<String>,    // Critic-specific
    pub min_score_threshold: u8,           // default: 7
    pub min_challenges: u32,               // default: 3
}
```

---

## Context Management & Agent Loop Integration

### Context Injection

Extends existing `ContextInjector` with inbox awareness:

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
- **Collaborative session invite**: strong prompt
- Agent reads details via `inbox_read` on demand (R8)

### Context Budget

Tools (`inbox_read`, `inbox_thread`, `session_read`) return full content. The existing agent loop context control strategy handles truncation — no pre-truncation at the tool level. This keeps context budget decisions in one place (the agent loop) rather than scattered across individual tools.

The context priority list below informs the agent loop's truncation order when budget is tight.

### Context Priority (high to low)

1. Current task instructions
2. to messages (actionable)
3. Tool results / work output
4. cc messages (informational)
5. Team digest (background)
6. <- truncation line adjusts dynamically based on token budget ->

### Message Lifecycle

```
Unread -> Read      (agent calls inbox_read)
Unread -> Expired   (TTL expiry: to 2h, cc 30m, system 15m — configurable)
       -> Archived  (team disbanded or session ended)
```

**TTL computation**: `MessageRouter` sets `expires_at = created_at + ttl` at send time. TTL determined by the highest-priority recipient role (to > cc > system). For messages with mixed to/cc recipients, use the longer TTL (to: 2h).

**Team disbandment**: All pending messages marked expired; active collaborative sessions cancelled.

### Event Log (Audit Layer)

All messages and operations persisted in event log with retention policy:

```rust
pub struct TeamEvent {
    pub id: String,
    pub team_id: String,
    pub event_type: TeamEventType,
    pub agent_id: String,
    pub payload: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

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

**Retention policy**: Event log entries pruned when team is disbanded (keep last 24h by default, configurable). For active teams, events older than `max_event_age` (default: 72h) are pruned on a lazy basis (checked during `team_digest` generation or periodic cleanup). This prevents unbounded growth while preserving enough history for digest generation and debugging.

---

## New Module Structure

```
src/teams/
├── mod.rs                  // re-exports
├── types.rs                // Team, TeamMember (existing, enhanced)
├── store.rs                // SqliteTeamStore (existing, enhanced)
├── artifacts.rs            // NEW: TaskArtifact, ArtifactType, storage
├── messages/
│   ├── mod.rs              // re-exports
│   ├── types.rs            // TeamMessage, Recipient, MessageType, MessageStatus
│   ├── router.rs           // MessageRouter: send, broadcast, route
│   ├── inbox.rs            // Inbox: read, peek, thread, expire
│   └── store.rs            // SQLite message storage
├── sessions/
│   ├── mod.rs
│   ├── types.rs            // CollaborativeSession, SessionTurn, SessionOutcome
│   ├── coordinator.rs      // SessionCoordinator: create, monitor, conclude
│   └── escalation.rs       // EscalationRule, auto-escalation logic
├── roles/
│   ├── mod.rs
│   ├── types.rs            // AgentRole, TeamRoleConfig
│   └── review.rs           // ReviewScore, DimensionScore, Challenge, validation
├── context.rs              // NEW: InboxContext, TeamContextBudget
└── events.rs               // NEW: TeamEvent, TeamEventType, event log store

builtin_tools/team/
├── delegate.rs             // existing, enhanced with artifact persistence
├── message_send.rs         // NEW: send messages (to/cc routing)
├── inbox_read.rs           // NEW: read inbox + thread mode
├── team_digest.rs          // NEW: generate team summary
├── task_submit.rs          // NEW: submit task artifacts
├── task_read_artifact.rs   // NEW: read artifacts
├── review_score.rs         // NEW: structured review with configurable validation
├── session_collaborate.rs  // NEW: start collaborative session
├── session_turn.rs         // NEW: respond or conclude in session
└── session_read.rs         // NEW: read transcript/outcome
```

## Delivery Phases

**Phase 1 — Layer 1 Enhancement** (low risk, foundation):
- TaskArtifact system + storage
- task_submit / task_read_artifact tools
- Task lifecycle events -> Layer 2 notifications (needs Phase 2, can stub)

**Phase 2 — Layer 2 MessageRouter** (core new feature):
- Message model + SQLite storage
- Inbox mechanism (read, thread, expire)
- message_send / inbox_read / inbox_thread / team_digest tools
- ContextInjector integration
- Wire up Layer 1 lifecycle events

**Phase 3 — Layer 3 CollaborativeSession**:
- Session model + coordinator
- Auto-escalation rules
- session_collaborate / session_respond / session_conclude / session_read tools

**Phase 4 — Role Mechanism**:
- AgentRole enum + TeamRoleConfig
- review_score tool with validation enforcement
- Explorer / Critic prompt templates
- Leader orchestration prompt template

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Task system | Unify on CoordTask, retire TeamTask | CoordTask has DAG/priority/metadata; no reason for two systems |
| Storage backend | SQLite (not file-based) | Single-host app; ACID; query capability; consistent with rest of Aleph |
| Routing model | to + cc (no bcc) | Simplicity; agents don't need hidden information flows |
| Message status | Derived from read_at + expires_at (not stored) | Avoids sync issues between message and recipient tables |
| Role behavior | Prompt-based (not code) | R8 LLM sovereignty; R10 intelligence in prompts |
| Escalation trigger | Suggestion to leader (not auto-action) | Leader (LLM) judges content quality, not just message count; R8 compliant |
| Session orchestration | Leader agent via tools (not code-level orchestrator) | R8; session is a data structure, leader drives the process |
| Context management | Tools return full content; agent loop truncates | Single truncation authority; no scattered pre-truncation |
| Critic validation | Configurable thresholds from TeamRoleConfig | Tool constraint (empowerment); flexible per task complexity |
| Tool consolidation | 9 new tools (merged inbox_read+thread, session_respond+conclude) | Fewer tools → better LLM tool selection accuracy |
| Event log retention | 72h active / 24h post-disband, lazy GC | Prevents unbounded growth; enough history for digests |
