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

| | Spawn | Delegate | Team | A2A |
|---|---|---|---|---|
| **Trigger** | LLM automatic | LLM automatic | User `/team` command | LLM automatic |
| **Tools** | `subagent_spawn/steer/kill` | `session_send` | 9 team tools (see below) | `a2a_delegate`, `a2a_agents` |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) | Per-call (one remote task) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) | Cross-process (Aleph → remote agent) |
| **Communication** | Return value | Messages (fire-and-forget or wait) | Three-layer: Tasks + Messages + Sessions | A2A protocol over HTTP (JSON-RPC 2.0) |

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

**Escalation**: Suggestion-based, not automatic. `EscalationRule` defines the thread-message threshold (default 5) and an on/off switch. When a reply thread exceeds the threshold, the `MessageRouter` sends one `SystemNotification` to the leader suggesting escalation. The leader (LLM) decides whether to actually escalate based on content, not just counts (R8). The threshold and switch are operator-tunable via the `[team_messages]` TOML section (`thread_message_threshold` / `escalation_enabled`), falling back to `EscalationRule::default()` per field when absent (a `0` threshold clamps to the default) — the message-router parallel to `[team_dispatcher]` (§4.4) and `[team_broadcast]` (§4.5), mapped at the `agent_init` boot site.

**Tools**:
- `session_collaborate` — start collaborative session (participants, topic, max_rounds)
- `session_turn` — speak in session (`mode: respond` or `mode: conclude` to propose ending with outcome)
- `session_read` — read transcript and outcome

### Review & Acceptance Mechanism

> Doc-drift fix (2026-07-19): an earlier revision of this section described
> `review_score` / `ReviewScore` / `TeamRoleConfig` / `min_challenges`, none of
> which exist in the implementation. What follows is the real mechanism.

Roles are prompt-level only (leader orchestration preamble in
`src/teams/leader_prompt.rs` + per-member handoff context); there is no
code-level role enum for members.

**Review**: the leader accepts/rejects a submitted deliverable with the
`task_review` tool (`src/builtin_tools/team/task_review.rs`) — approve →
`Completed` (dependents unblock), reject → back to `InProgress` (redo in
place, feedback rides along). Verdicts are recorded on the task run
(`ReviewVerdict` / `ReviewerKind` in `src/agents/swarm/tasks/mod.rs`).

**Acceptance contract**: per-task policy lives in the task `metadata` JSON
channel (`src/agents/swarm/tasks/acceptance.rs`) — `acceptance_criteria`
(definition-of-done checklist rendered into the handoff prompt and the review
gate), `lead_review_required` (route successful runs to `WaitingReview`), and
`require_grounding` (approvals must carry reviewer-side measurement evidence).

**Grounding evidence** (2026-07-19): `task_review` accepts a structured
`grounding` field (`kind: exit_code | numeric | line_count` — the same closed
truth vocabulary as loop_graph anchors — plus `source`/`value`/`note`). When
the task metadata carries `require_grounding: true`, an approve without
evidence bounces with status `grounding_required`. Evidence is persisted as a
`[grounding]` task comment for later audit. Reviewers may collect evidence
independently via `subagent(agent_type="loop-auditor")` (fresh-context
measure-only builtin). See `docs/reference/GRAPH_LAYER.md` §多智能体融合.

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
    SessionStarted,
    SessionConcluded,
    SessionDeadlocked,
    DigestGenerated,
    ShutdownRequested,
    ShutdownResolved,
    PlanSubmitted,
    PlanResolved,

    // Team lifecycle events (EventBus integration)
    TeamCreated,
    MemberAdded,
    MemberRemoved,
    TaskAssigned,
    TaskUpdated,
    TeamDisbanded,
}
```

**Retention**: Active teams — events older than 72h pruned lazily (during `team_digest` generation or periodic cleanup). Disbanded teams — events older than 24h pruned. Used for `team_digest` generation.

## Mode 4: A2A (Remote Agent Delegation)

**Tools**: `a2a_delegate`, `a2a_agents`

Modes 1–3 are all **in-process** — every agent runs inside the same
`aleph-server`. Mode 4 reaches *outside* the process: it delegates a task to a
**remote agent** that speaks the A2A protocol (Agent-to-Agent — JSON-RPC 2.0
over HTTP, with Agent Card discovery).

Aleph has always *served* A2A — it exposes its own Agent Card at
`/.well-known/agent-card.json` and answers `message/send` / `tasks/*` (including
`tasks/pushNotificationConfig/*` and `agent/getAuthenticatedExtendedCard`) on
`/a2a`. Registered push-notification webhooks now fire on every task
status/artifact update (delivered fire-and-forget alongside the SSE stream), so
clients that are not attached to the live stream still receive task updates.
Mode 4 wires the **outbound** half: Aleph as an A2A *client*. (Previously the
outbound stack — `A2AClient`, `SmartRouter`, `A2ASubAgent` — was constructed at
startup and immediately dropped; it was unreachable until these tools.)

- **`a2a_agents`** — manage the set of known remote agents: `list` them,
  `add` one by URL (its Agent Card is fetched so smart routing learns its
  skills), or `remove` one. Remotes can also be pre-declared in the `[a2a]`
  config section.
- **`a2a_delegate`** — hand a self-contained task to a remote agent. The
  `SmartRouter` (exact name → exact skill → LLM-semantic) picks the best
  agent, or the caller pins one explicitly via the `agent` argument.

**Runtime**: `a2a_delegate` → `A2ASubAgent::execute_delegation` → `SmartRouter`
routing → `A2AClientPool` (pooled per-agent HTTP clients) → remote `/a2a`
JSON-RPC. The outcome is also recorded as a `RawMemory(Delegation)` row so the
parent agent's long-term memory can distil lessons from it.

**When LLM uses it**: tasks better handled by a specialised external agent —
another Aleph instance, a colleague's agent, or any A2A-compliant agent on
another host.

**Config**: A2A is off by default. Set `[a2a] enabled = true`; the `a2a_*`
tools register only when it is enabled.

### Outbound transport

Outbound delegation is streaming-first: `a2a_delegate` POSTs to the remote
agent's `/a2a/stream` (SSE) endpoint, consuming `status-update` /
`artifact-update` events with a 90s idle-timeout for liveness and live
progress notifications. A remote agent without a streaming route (non-Aleph
A2A agents) is handled by a transparent fallback to the synchronous
`message/send` endpoint.

Config-declared agents (`[[a2a.agents]]`) start with a placeholder Agent Card.
A one-shot background task at server startup fetches each agent's real Agent
Card (skills, description, version) and replaces the placeholder, so smart
routing and `a2a_agents list` see real skill data.

## MoA (Mixture of Agents)

Aleph has two independent MoA surfaces. They are complementary, not
overlapping — see the distinction at the end.

### Continuous Advisory (this port)

Ported from hermes-agent's `MoAClient` (spec:
`docs/superpowers/specs/2026-07-05-moa-continuous-advisory-port-design.md`).
`MoaProvider` (`src/providers/moa/provider.rs`) is a virtual `AiProvider`
facade that sits in front of the acting model: each Think iteration it
flattens the live conversation into a tool-free advisory view
(`advisory_view.rs`, UTF-8-safe head+tail truncation, always ends on a user
turn), fans it out in parallel to the preset's advisor models with a
per-advisor timeout (fail-soft — a timed-out/erroring advisor degrades to a
`[timeout after Ns]` / `[failed: ...]` note, never breaks the turn), attaches
the combined guidance at the **end** of the aggregator's prompt
(`prompts.rs::attach_guidance` — appending, not inserting mid-transcript,
keeps the `[system][task][tool-history]` prefix byte-stable and KV-cache
reusable), then calls the aggregator, which IS the acting model. The harness
sees one ordinary provider (R10); none of this touches `src/harness/`.

- **Config**: `[moa]` (`src/config/types/moa.rs`) — named presets
  (`advisors: Vec<MoaSlot>` + `aggregator: MoaSlot`), `fanout` cadence
  (`per_iteration` re-runs advisors every tool iteration, hermes default;
  `user_turn` runs once per run and reuses the advice),
  `advisor_timeout_secs`, `advisor_max_tokens`/temperatures,
  `default_preset`, `save_traces`.
- **Activation**: `moa` builtin tool (`src/builtin_tools/moa_manage.rs`,
  operator-tier gated via `method_authz`) — `on`/`off`/`once`/`status`/
  `list`/`set_preset`/`delete_preset`. Per-session state lives in
  `src/providers/session_moa_handle.rs` (sticky vs one-shot; `take_for_run`
  consumes a one-shot pref atomically — the single restore point, so
  success, error and cancel paths all leave no state behind). `/moa
  <prompt>` is a one-shot intercept on both the panel and channel dispatch
  paths (excluded from the L0 fast path, like `/loop`).
- **Precedence** (`runner_impl.rs` Step 3-MoA, run construction): MoA >
  `select_model` session pick > agent `model_hint` pin > flow `BrainRef`
  brain. An unusable preset (no `[moa]` section, unresolvable provider) fails
  soft — the run falls back to the normal provider chain and logs a warning.
- **Accounting**: advisor usage is metered per-slot (`MeteringProvider`
  labelled `moa:<idx>:<provider>:<model>`) and kept OUT of
  `ProviderResponse.usage` so the context gauge stays honest; a summed
  `MoaAdvisorSpend` event restores visibility. Four `LoopTraceEvent`
  variants — `MoaAdvisor`, `MoaAggregating`, `MoaAdvisorSpend` (live) and
  `MoaTurnTrace` (persist-only, gated by `save_traces`) — are the harness's
  only touchpoint (`src/harness/trace.rs`). The panel renders the three live
  events inline as reasoning blocks (◇ 顾问 / ◆ 聚合 / ▫ 开销,
  `interfaces/webchat/src/platform/wide/views/chat/events.rs`).

**Round 2** (spec `docs/superpowers/specs/2026-07-05-moa-round2-optimization-design.md`)
added four pieces on top of the port above:

- **Selector integration**: `select_model` accepts `"moa:<preset>"` (or bare
  `"moa"` for the default preset) as a peer slot to normal model picks —
  implemented inline in the tool itself (`src/builtin_tools/select_model.rs`
  arms MoA sticky and clears `session_model_handle`; picking a normal model
  clears MoA sticky instead; the two slots are mutually exclusive). A
  separate helper, `apply_moa_selector_semantics`
  (`src/gateway/handlers/agent.rs`), does the analogous arm/clear for an
  explicit `model_override{provider:"moa"}` picked via the chat-window model
  picker (Panel) — it does not back `select_model`'s own inline logic. It is
  applied at both `chat.send`/`agent.run` call sites: the Simulated-fallback
  `AgentRunManager::start_run` (same file) and, since the Round-2 Task 18
  fix, the real-`ExecutionEngine` path's `handle_chat_send_with_engine`
  (`src/bin/aleph-server/server_init.rs`) — closing a gap where a Panel
  "moa" pick silently did nothing in any real deployment (any provider
  configured). The synthesized `[voice]` low-TTFT pin is exempt from this
  exclusion (config-derived, not user intent — it must never clear an armed
  MoA session). The same presets ride `providers.catalog` as a synthetic
  `"moa"` provider row (`src/gateway/handlers/providers/handlers.rs`) and
  `list_models`'s `moa_presets` field (`src/builtin_tools/list_models.rs`),
  so Panel/CLI pickers see MoA presets without a new RPC.
- **Advisor prompt-cache + multimodal**: `mark_cache_breakpoints`
  (`src/providers/moa/advisory_view.rs`) marks an Anthropic ephemeral
  `cache_control` on the last text block of each of the last three
  advisory-view messages, so `per_iteration` fanout replays the cached
  prefix instead of re-billing it every tool step (hermes measured 0/1227
  cache reads without this). Image content renders as a `"[image: <mime>]"`
  placeholder in the advisor view instead of being silently dropped.
- **Audit replay**: when `save_traces = true`, the persisted `MoaTurnTrace`
  event now carries the aggregator's own output/status
  (`aggregator_output`/`aggregator_status`, emitted only after the
  aggregator returns) alongside the full advisor I/O. `trace.by_runs` REPLAY
  surfaces a one-line summary (`shared/protocol/src/trace_presentation.rs`)
  and the panel's `moa_turn_trace` reasoning block (`events.rs`) renders the
  full advisor-by-advisor transcript plus the aggregator's answer; none of
  this reaches the live wire.
- **Overhead bucket**: advisor spend across all sessions aggregates into a
  single `"moa-advisors"` bucket (`aggregate_moa_advisor_usage`,
  `src/resilience/database/traces.rs`) instead of one synthetic "agent" per
  advisor slot, keeping team/session usage views honest.
- **Panel visual config** (Round 4, 2026-07-06; deepened 2026-07-12): a
  visual `[moa]` preset editor at Settings → MoA
  (`interfaces/webchat/src/platform/wide/views/settings/moa/`) — preset
  cards, a create/edit form whose advisor/aggregator dropdowns are
  constrained to already-configured, credentialed models
  (`options::available_options` over `providers.catalog`, with the synthetic
  `"moa"` pseudo-row filtered out), advanced knobs, and a global
  `save_traces` toggle. Writes go through the gateway RPCs
  `moa.{listPresets,savePreset,deletePreset,setDefault,setSaveTraces}`
  (`src/gateway/handlers/moa.rs`) backed by the single write-core
  `MoaPresetStore` (`src/providers/moa/preset_store.rs`) — the same core the
  `moa` tool uses, so tool and panel never diverge. Each card shows the
  per-turn model-call count and a "Use in chat" action that arms the preset
  on the chat session's model selector (`ModelOverride{provider:"moa"}`); a
  "Duplicate" action clones a preset as a starting point. Activation itself
  stays session-scoped (chat model picker + `moa` tool); the settings page
  only edits config (R4 kept it a pure config surface).

### One-Shot Task Fan-Out (existing, previously undocumented)

Independent of the port above, the `subagent` tool has always supported a
Mixture-of-Agents shorthand: `proposer_models` (replicate the top-level
`task` across models as parallel proposers) + `synthesize` (run ONE
aggregator sub-agent that folds the proposals into a single answer) +
`aggregator_model` (`src/agents/subagent_tool/` — see `parse.rs` /
`loop_tool.rs`; Wang et al., "Mixture-of-Agents Enhances Large Language Model
Capabilities", 2406.04692). `synthesize` requires a foreground batch
(`run_in_background=false`) and returns `status: "moa_completed"` with a
`synthesis` field plus the raw `results`.

### The distinction

The continuous port watches the *live conversation* and advises the acting
model turn-by-turn via a raw provider call (no tool registry, no harness
loop) — user-controlled (`/moa`, the `moa` tool) and cache-friendly by
design. The `subagent` shorthand fans a *fresh, self-contained task* out to
several full sub-agent harnesses and reduces once at the end — model-
initiated (the LLM decides to call `subagent`), each proposer gets its own
isolated context. Use the port for a standing second opinion on the
conversation Aleph is already having; use `subagent`
`proposer_models`+`synthesize` for several independent takes on a one-off
task.

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

### A2A (Remote Agents)
| Tool | Description |
|------|-------------|
| `a2a_delegate` | Delegate a task to a remote agent over the A2A protocol (auto-routed, or pinned via `agent`) |
| `a2a_agents` | List / add / remove the remote A2A agents Aleph can delegate to |

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
| `task_review` | Leader approves/rejects a submitted task deliverable — approve completes the task and unblocks dependents, reject sends it back for in-place redo; supports optional `grounding` evidence, required when the task carries `require_grounding: true` |

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
│   ├── schedule/
│   │   ├── mod.rs           // dispatch_once, run_task, persist_artifact
│   │   ├── select.rs        // select_schedulable, is_zombie, is_stale_review
│   │   ├── failure.rs       // fail_or_retry (bounded retry + backoff)
│   │   └── reclaim.rs       // reclaim_zombies / reclaim_orphaned / warn_stale_reviews
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
- **Review gating**: `task_review` requires `grounding` evidence for an approval only when the task's metadata sets `require_grounding: true` — a plain metadata flag, not a role config type. Roles themselves stay prompt-level only (leader orchestration preamble + per-member handoff context) — there is no code-level role config type
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

`check_status` is **non-destructive** — a completed sub-agent's outcome stays
queryable until the TTL prune (1h), so the parent may poll the same
`request_id` more than once without it vanishing.

When status == "running", the response carries elapsed time, a derived
activity `summary`, and the recent `progress` events:

```json
{
  "status": "running",
  "request_id": "...",
  "task": "...",
  "elapsed_secs": 12,
  "summary": { "steps": 1, "last_activity": "tool_called", "last_tool": "grep" },
  "progress": [
    { "step": 0, "kind": "llm_thinking", ... },
    { "step": 1, "kind": "tool_called", "tool_name": "grep", ... },
    { "step": 1, "kind": "tool_returned", "latency_ms": 42, "preview": "...", ... }
  ]
}
```

Up to 10 most-recent progress events are returned. The buffer caps at 50
internally; older events are evicted FIFO.

When status == "completed", the response carries the same run metrics the
foreground spawn path returns — `iterations`, `tool_calls_made`,
`total_tokens` — plus `duration_secs`:

```json
{
  "status": "completed",
  "request_id": "...",
  "task": "...",
  "result": "...",
  "iterations": 4,
  "tool_calls_made": 9,
  "total_tokens": 555,
  "duration_secs": 31
}
```

A failed background sub-agent surfaces as a `ToolResult::Error`.

### wait Action (event-driven blocking)

`check_status`/`list` are poll actions — each call costs the parent a full LLM
turn. When the parent must block on a specific result before continuing, the
`wait` action parks on the tracker's completion notifier instead of spin-polling:

- `{"action": "wait", "request_id": "...", "timeout_secs": 120}` — block until
  that sub-agent finishes or the bounded window elapses. On completion it returns
  the **same shape** `check_status` gives (a failure still surfaces as a
  `ToolResult::Error`); on timeout it returns `{"status": "still_running",
  "request_id": "...", "elapsed_secs": N, "waited_secs": 120, ...}` so the model
  may `wait` again.
- `{"action": "wait", "request_ids": ["a", "b", ...], "timeout_secs": 120}` —
  fan-out first-completion: returns as soon as **any** id in the set finishes,
  reporting which one (`request_id`). Drain the rest by dropping it and calling
  `wait` again. A failed first-completion comes back as a `status: "failed"`
  Success (not a `ToolResult::Error`) so it does not trip the harness failure
  counter — the model sees which child failed and can wait for the rest. Mirrors
  codex `wait_agent`.

`timeout_secs` defaults to 120 and is clamped to 600 (`DEFAULT_WAIT_TIMEOUT_SECS`
/ `MAX_WAIT_TIMEOUT_SECS` in `types.rs`) — well under the subagent tool's
`1_800_000`ms wall-clock budget, so a single `wait` can never hang the turn.

Implementation (`src/agents/background_tracker.rs`): a `tokio::sync::Notify`
`completion` signal fires at the end of `mark_completed`; `wait`/`wait_any` use
the same `Notified::enable` arm-before-check loop as
`builtin_tools::process_registry::wait` (no lost wakeup, no busy-poll). This
mirrors the bash-background `wait` primitive, closing the asymmetry where only
shell background jobs had an efficient blocking wait. To keep a concurrent waiter
from ever seeing a completing agent as `NotFound`, `mark_completed` now inserts
into `completed` **before** removing from `running` (the id is never absent from
both maps); `flat_nodes` de-dupes by id to tolerate the brief double-presence.

**Announce dedup.** A result delivered on-demand (via `wait`, or a `check_status`
that returned the outcome) is marked consumed
(`BackgroundAgentTracker::mark_consumed`); the proactive `subagent_announce` (R5)
checks `is_consumed` and skips re-delivering what the model already saw — so the
poll/wait path and the announce path never double-inject the same result (hermes
`_completion_consumed` parity). Both paths share the process-global
`BackgroundAgentTracker::global()` instance.

### list Action

`{"action": "list"}` enumerates every background sub-agent the tracker still
holds — running and recently-completed — so the parent can recover a
`request_id` it no longer has in context:

```json
{
  "running":   [ { "request_id": "...", "task": "...", "elapsed_secs": 12 } ],
  "running_count": 1,
  "completed": [ { "status": "completed", "request_id": "...", "task": "...",
                   "result": "...", "iterations": 4, "duration_secs": 31, ... } ],
  "completed_count": 1
}
```

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

File tools are isolated too: the spawner publishes a per-run
`tools::fs_scope::FsScope` task-local around the child harness, so
`file_read` / `file_write` / `file_edit` / `apply_patch` / `file_ops`
resolve relative paths at the worktree root and **rebase parent-repo
absolute paths into the checkout** (the deny gate evaluates the post-rebase
target). The team dispatcher gets the same guarantee through a different
seam: `execute_member_task` points the member's `workspace_override` at the
worktree, and `run_agent_loop` roots that run's `FsScope` / `ToolContext`
there. Cross-agent same-path mutations (parent + non-isolated subagent
sharing a workspace) are serialized by `tools::path_locks` — a process-wide
per-canonical-path mutex held across each mutating tool's read-modify-write
critical section.

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
