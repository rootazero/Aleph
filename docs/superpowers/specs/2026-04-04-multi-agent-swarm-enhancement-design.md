# Multi-Agent Swarm Enhancement Design

**Date:** 2026-04-04
**Status:** Approved
**Scope:** Enhance Aleph's multi-agent collaboration system — Spawn mode upgrade, swarm TODO completion, code cleanup

---

## 1. Background

### 1.1 Current State

Aleph has a three-mode multi-agent collaboration system:

- **Spawn Mode** (`src/agent_loop/subagent_tool.rs`): Ephemeral anonymous sub-agents with background execution support via `tokio::spawn`
- **Delegate Mode** (`src/builtin_tools/sessions/send_tool.rs`): Peer-to-peer messaging between named agent sessions
- **Team Mode** (`src/teams/` + `src/builtin_tools/team/`): Structured coordination with DAG tasks, messaging, collaborative sessions, and roles

Supporting infrastructure:
- **Swarm Sensing** (`src/agents/swarm/`): Three-tier event bus (Critical/Important/Info), semantic aggregator, context injector, collective memory
- **Team Messages** (`src/teams/messages/`): SQLite-backed persistent messaging with threading, escalation, and TTL

### 1.2 Gaps Identified

Compared to Claude Code's multi-agent system and Aleph's own design aspirations:

1. **Spawn mode is anonymous and isolated** — sub-agents cannot communicate with each other or share tasks. Claude Code fills this gap with "teammates" (named, communicable, task-sharing agents).
2. **Three swarm TODOs unfinished:**
   - `aggregator.rs:228` — LLM intelligent summarization not integrated
   - `context_injector.rs:202` — Task context injection returns `None` (teams module ready but not wired)
   - `context_injector.rs:226` — Critical event interrupt mechanism is a stub
3. **`subagent_tool.rs` is 26KB** — multiple responsibilities need separation

### 1.3 Reference Analysis: Claude Code

Claude Code's teammate system provides:
- In-process concurrent execution (AsyncLocalStorage isolation)
- File-based mailbox communication (JSON files with file locking)
- Shared task list coordination
- Git worktree isolation for parallel file operations
- Permission synchronization between leader and workers

Key insight: Claude Code's teammates fill the gap between "anonymous one-shot sub-agent" and "heavyweight formal team" — lightweight named agents that can communicate and share work.

---

## 2. Design Decisions

### D1. Enhancement Strategy: Top-Down (User Value First)

Sequence: Spawn enhancement → Code cleanup → Swarm TODO completion.
Rationale: Spawn enhancement delivers the most user-visible improvement immediately.

### D2. Spawn Enhancement over New Concept

Enhance existing `SubagentTool` with optional `name` and `team_name` fields rather than introducing a new "Teammate" concept.
Rationale: Zero new tool names, backward compatible, LLM already knows "subagent."

### D3. Reuse Team Messages for Communication

Named teammates communicate via existing `TeamMessages` (SQLite-backed) rather than a new channel-based mailbox.
Rationale: Aleph already has a production-ready messaging system with threading, TTL, and escalation. No need to duplicate.

### D4. Two Communication Layers, Separate Purposes

- **Team Messages (SQLite)**: Intentional agent-to-agent dialogue (agent calls `message_send` tool)
- **Swarm Bus (broadcast channel)**: Automatic system-level sensing (events published by infrastructure)
- Both converge in `ContextInjector` which injects swarm state + inbox context into agent prompts

### D5. No Git Worktree Isolation This Iteration

Scope already includes Spawn enhancement + 3 TODO completions + code cleanup. Worktree isolation deferred to a future iteration.

---

## 3. Architecture

### 3.1 Phase Overview

```
Phase 1: Enhance Spawn Mode (named teammates + team integration)
Phase 2: Clean up subagent_tool.rs (split into focused modules)
Phase 3: Wire context_injector to teams module
Phase 4: Implement critical event interrupt mechanism
Phase 5: Integrate LLM aggregator (lowest priority)
```

### 3.2 Communication Architecture (Post-Enhancement)

```
┌─────────────────────────────────────────────────────┐
│                    Agent Think Phase                  │
│  ┌─────────────────────────────────────────────────┐ │
│  │ ContextInjector.inject_swarm_state()            │ │
│  │  ├── Swarm State (from event bus)               │ │
│  │  ├── Task Context (from CoordTaskStore)         │ │
│  │  └── Inbox Context (from InboxContextProvider)  │ │
│  └─────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────┘
         ▲                              ▲
         │                              │
    Swarm Bus                    Team Messages
  (broadcast channel)          (SQLite persistent)
  Auto-published events      Agent-initiated messages
  Fire-and-forget             Threading, TTL, escalation
```

---

## 4. Phase 1: Enhance Spawn Mode

### 4.1 RunArgs Extension

Current `RunArgs`:
```rust
struct RunArgs {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    run_in_background: bool,
    context_summary: Option<String>,
}
```

Add two optional fields:
```rust
struct RunArgs {
    // ... existing fields unchanged ...

    /// Optional name — named agents are addressable via message_send / inbox_read
    name: Option<String>,
    /// Optional team name — agents with same team_name share task list and messages
    team_name: Option<String>,
}
```

Named teammates automatically get team messaging capabilities via injected tools. No explicit enable/disable flag needed — if you have a name and team, you get messages.

Behavior matrix:

| name | team_name | Behavior |
|------|-----------|----------|
| None | None | Current anonymous sub-agent (unchanged) |
| Some | None | Named agent, addressable but no shared tasks |
| Some | Some | Full teammate: named, shared tasks, shared messages |
| None | Some | Invalid — team requires a name (reject with error) |

### 4.2 SubagentAction Extension

Add `SendMessage` variant to the existing action enum:

```rust
enum SubagentAction {
    Run(RunArgs),
    CheckStatus(String),
    /// Send a message to a named teammate
    SendMessage { to: String, text: String },
    /// Read inbox for the calling agent
    ReadInbox { team_name: String },
}
```

This keeps teammate communication within the `subagent` tool (no new tool needed).

### 4.3 Teammate Registration Flow

When `SubagentTool` receives a `Run` with `name` + `team_name`:

1. **Auto-create lightweight team** if it doesn't exist (via `TeamStore`)
2. **Register the agent as team member** (via `TeamStore::add_member`)
3. **Inject team tools** into the sub-agent's tool registry:
   - `message_send`, `inbox_read` (from `src/builtin_tools/team/`)
   - `task_create`, `task_update`, `task_list` (task coordination)
4. **Register CancellationToken** in parent's background tracker
5. **Spawn via `tokio::spawn`** (always background for named teammates)

### 4.4 Teammate Lifecycle

```
Parent calls subagent(name="researcher", team_name="analysis") 
  → Auto-create team "analysis" if needed
  → Register "researcher" as member
  → Inject team tools into researcher's tool registry
  → tokio::spawn the agent loop
  → Return { status: "running", name: "researcher", team_name: "analysis" }

Parent calls subagent(action="send_message", to="researcher", text="...")
  → Route through TeamMessages (MessageRouter → SqliteMessageStore)
  
Researcher's ContextInjector injects inbox context before Think phase
  → Researcher sees the message, can reply via message_send tool

Researcher completes task
  → Removed from team
  → CancellationToken cleaned up
```

### 4.5 Tool Schema Changes

The `subagent` tool JSON Schema gains:

```json
{
  "properties": {
    "name": {
      "type": "string",
      "description": "Optional name for the sub-agent. Named agents are addressable and can communicate."
    },
    "team_name": {
      "type": "string",
      "description": "Team name for shared tasks and messages. Requires name."
    },
    "action": {
      "type": "string",
      "enum": ["run", "check_status", "send_message", "read_inbox"],
      "description": "Action to perform. Default: run."
    },
    "to": {
      "type": "string",
      "description": "Recipient agent name (for send_message action)."
    },
    "text": {
      "type": "string",
      "description": "Message text (for send_message action)."
    }
  }
}
```

---

## 5. Phase 2: Code Cleanup

### 5.1 File Split

Current:
```
src/agent_loop/
├── subagent_tool.rs        (26KB, mixed responsibilities)
└── background_tracker.rs   (existing, clean)
```

After:
```
src/agent_loop/
├── subagent_tool.rs        (~200 lines: LoopTool impl, param parsing, action dispatch)
├── subagent_runner.rs      (new: run_subagent(), AgentDef construction, context injection)
├── subagent_teammates.rs   (new: teammate registration, team auto-creation, tool injection)
└── background_tracker.rs   (unchanged)
```

### 5.2 Responsibilities

- **`subagent_tool.rs`**: Parse JSON input → determine `SubagentAction` → dispatch to runner or teammates module → format result
- **`subagent_runner.rs`**: Build `AgentDef` from args → create `LoopConfig` → call `AgentLoop::run()` → return result
- **`subagent_teammates.rs`**: Team auto-creation → member registration → tool registry injection → lifecycle management

### 5.3 What Not To Do

- Do not rename `subagent` to `teammate` — backward compatibility
- Do not merge with `sessions_send` — different use cases
- Do not delete `BackgroundAgentTracker` — already clean and focused

---

## 6. Phase 3: Context Injector + Teams Integration

### 6.1 Wire `inject_task_context()`

Current state: returns `None` with a TODO comment.

Implementation:
```rust
pub async fn inject_task_context(&self, agent_id: &str) -> Option<String> {
    let store = self.task_store.as_ref()?;
    let tasks = store.list_by_owner(agent_id).await.ok()?;

    if tasks.is_empty() {
        return None;
    }

    let mut ctx = String::from("## Your Tasks\n");
    for task in &tasks {
        ctx.push_str(&format!(
            "- [{}] {} (priority: {:?})\n",
            task.status, task.subject, task.priority
        ));
    }
    Some(ctx)
}
```

### 6.2 Wire at Initialization

In `SwarmCoordinator` initialization, pass the `CoordTaskStore` and `InboxContextProvider` instances to `ContextInjector`:

```rust
let injector = ContextInjector::new(bus.clone())
    .with_task_store(task_store.clone())
    .with_inbox_provider(inbox_provider.clone());
```

The builder methods already exist — this is pure wiring work.

### 6.3 Verify InboxContextProvider

Confirm that `InboxContextProvider::get_inbox_context()` in `src/teams/context.rs` is implemented and returns meaningful data. If it's a stub, implement it to query `SqliteMessageStore::read_inbox()`.

---

## 7. Phase 4: Critical Event Interrupt Mechanism

### 7.1 Approach: Cooperative Cancellation

Use Aleph's existing `CancellationToken` pattern (already used in background sub-agents).

### 7.2 New Fields in ContextInjector

```rust
pub struct ContextInjector {
    // ... existing fields ...
    /// agent_id → CancellationToken for interrupting current LLM call
    agent_tokens: DashMap<String, CancellationToken>,
    /// agent_id → pending interrupt feedback to inject
    pending_interrupts: DashMap<String, String>,
}
```

### 7.3 Interrupt Flow

1. Critical event arrives via Swarm Bus subscription
2. `handle_critical_event()` looks up target agent's `CancellationToken` via `agent_tokens`
3. Calls `token.cancel()` — cooperatively cancels the in-flight LLM call
4. Stores formatted interrupt message in `pending_interrupts`
5. Agent loop detects cancellation, checks `pending_interrupts`
6. Injects interrupt message as system feedback in next Think phase
7. Agent re-enters Think phase with awareness of the critical event

### 7.4 Token Registration

- Sub-agent spawn: `injector.agent_tokens.insert(agent_id, cancel_token.clone())`
- Sub-agent completion: `injector.agent_tokens.remove(&agent_id)`
- Reuses the same `CancellationToken` from `BackgroundAgentTracker`

### 7.5 Why Cooperative, Not Abort

`CancellationToken.cancel()` is cooperative — the agent loop can finish its current step gracefully before handling the interrupt. `abort()` would lose partial LLM output and leave resources in an inconsistent state.

---

## 8. Phase 5: LLM Aggregator Integration

### 8.1 Extend IntelligenceLayer

```rust
pub struct IntelligenceLayer {
    summary_interval: Duration,
    min_events: usize,
    /// AI provider for LLM summarization
    provider: Arc<dyn AiProvider>,
    /// Model hint for cost control (prefer haiku-tier)
    model_hint: Option<String>,
}
```

### 8.2 Summarization Flow

1. Timer ticks (default: 30s interval)
2. Read recent events from sliding window (up to 100)
3. If fewer than `min_events` (default: 10), skip
4. Format events into prompt: "Summarize the following agent activity in 2-3 sentences: ..."
5. Call `provider.complete()` with haiku-tier model
6. Publish `ImportantEvent::SwarmStateSummary` to bus

### 8.3 Cost Control

- Fixed interval (30s) — does not scale with event frequency
- `min_events` threshold — no LLM call when activity is low
- Cheapest model via `model_hint` — haiku-tier is sufficient for summarization
- Summary kept to 2-3 sentences — minimal token output

### 8.4 Priority: Lowest

This phase is optional for this iteration. The swarm sensing system functions without LLM summarization (rule-based aggregation still works). Implement only if time permits.

---

## 9. Code Cleanup Guidelines

### 9.1 Dead Code Removal

After each phase, check for:
- Unused imports from refactored modules
- Functions that moved to new files but weren't removed from the original
- Test cases that reference moved functions

### 9.2 No New Abstractions Without Justification

Per P6 (Simplicity) and the three-times rule:
- Do not create trait abstractions for teammate registration if only one implementation exists
- Do not add feature flags — all phases compile into the same binary
- Do not add backward-compatibility shims for renamed functions

### 9.3 File Size Targets

- `subagent_tool.rs`: ~200 lines (down from 26KB)
- `subagent_runner.rs`: ~200 lines
- `subagent_teammates.rs`: ~200 lines
- No new file should exceed 500 lines (per P2: High Cohesion)

---

## 10. Testing Strategy

### 10.1 Phase 1 Tests

- **Unit**: `RunArgs` parsing with name/team_name combinations
- **Unit**: Invalid combination (team_name without name) returns error
- **Integration**: Spawn named teammate → verify team auto-creation
- **Integration**: Send message to teammate → verify delivery via inbox_read

### 10.2 Phase 2 Tests

- **Unit**: Existing tests continue to pass after file split (move tests with code)

### 10.3 Phase 3 Tests

- **Unit**: `inject_task_context()` returns formatted task list
- **Unit**: Empty task list returns `None`
- **Integration**: SwarmCoordinator correctly wires task store and inbox provider

### 10.4 Phase 4 Tests

- **Unit**: `handle_critical_event()` cancels target agent's token
- **Unit**: Pending interrupt is stored and retrievable
- **Integration**: Critical event → agent loop detects cancellation → re-enters Think phase

### 10.5 Phase 5 Tests

- **Unit**: IntelligenceLayer skips when below min_events
- **Unit**: Summary prompt is correctly formatted
- **Integration**: LLM summary published as ImportantEvent::SwarmStateSummary

---

## 11. Architectural Compliance

| Red Line | Compliance |
|----------|------------|
| R8 LLM Sovereignty | Agent coordination driven by LLM tool selection, not hardcoded orchestration |
| R9 Everything is a Tool | Teammate spawn, messaging, task coordination all exposed as tool actions |
| R10 Intelligence in Prompt | Role behavior, task assignment via prompt injection (ContextInjector) |
| P1 Low Coupling | Phases are independent modules; Swarm Bus and Team Messages are separate layers |
| P2 High Cohesion | File split ensures each module has single responsibility |
| P4 Dependency Inversion | ContextInjector depends on `CoordTaskStore` trait, not concrete SQLite impl |
| P6 Simplicity | Reuses existing Team Messages instead of building new mailbox |

---

## 12. Out of Scope

- Git worktree isolation for parallel file operations
- Cross-team coordination (agents in multiple teams)
- Dynamic role adjustment mid-execution
- Pane-based visible execution (tmux/iTerm2 equivalent)
- Permission synchronization between leader and workers
