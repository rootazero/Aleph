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
| **Tools** | `subagent` (actions: run/batch/wait/…) | `session_send` | 9 team tools (see below) | `a2a_delegate`, `a2a_agents` |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) | Per-call (one remote task) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) | Cross-process (Aleph → remote agent) |
| **Communication** | Return value | Messages (fire-and-forget or wait) | Three-layer: Tasks + Messages + Sessions | A2A protocol over HTTP (JSON-RPC 2.0) |

## Mode 1: Spawn (Sub-Agent Dispatch)

**Tool**: the single `subagent` tool — actions `run` (default; `batch_tasks`
for parallel fan-out, MoA mode for mixture-of-agents), `check_status`, `wait`,
`cancel`, `list`, `send_message`, `read_inbox`.

The main agent spawns an ephemeral sub-agent to handle a focused sub-task. The
sub-agent has its own tool registry (excluding subagent tools to prevent
recursion), token budget, and timeout. It returns a result and is destroyed.

**Runtime**: Sub-agent spawning routes through Orchestrator → AgentHarness
(Phase 7, 2026-04-21). The legacy `src/agent_loop/` directory has been deleted.

**When LLM uses it**: Tasks benefiting from isolated context — parallel searches, code analysis, format conversion, translation.

**Swarm integration**: Sub-agent events are NOT published to the Event Bus (ephemeral, not a named agent).

### Only spawnable agents may be spawned (2026-08-08)

`agent_type` resolves through **`AgentRegistry::resolve_spawnable()`**, not `resolve()`. The two are not interchangeable and the gap was a real privilege escalation: the prompt-side catalog of delegatable agents filters on `mode == SubAgent`, while `resolve()` does not, so `agent_type="main"` resolved the builtin Primary definition and the child ran with `allowed_tools: ["*"]` — the exact set the sub-agent registry exists to withhold. Verified reproducible before the fix. All **four** spawn faces go through `resolve_spawnable()`: single `run`, a `batch_tasks` row, batch inheritance, and the MoA aggregator. A new spawn face that resolves its own agent type is a new instance of this bug.

### Surviving a daemon restart (2026-08-08)

`BackgroundAgentTracker` is process memory. Background children are additionally mirrored to disk by **`src/agents/background_persistence.rs`**, so a restart no longer erases them:

- `<data_dir>/background_subagents/<slug>/state.json` — the run record, written exactly twice (start, terminal), so the atomic tempfile+fsync+rename cost is bounded.
- `…/result.txt` — an append-only `<unix_ms>\t<text>` activity trail. `last_activity` is the timestamp of its last line, single-sourced rather than duplicated into `state.json` (a field rewritten on every progress event would cost one fsync per tool call).
- Boot reconcile writes a **terminal tombstone** for orphans instead of deleting the row — a mechanism that only records "it finished" cannot tell "it never ran" apart from "it ran and the write was lost". Without this, `check_status` on a child that died with the previous daemon returned `retryable: false` and the text `"No background sub-agent found with request_id '…'"`, which is indistinguishable from a typo and throws away whatever the child had produced.
- **Every byte written here goes through `SecretMasker`, unconditionally.** `result.txt` is the first place a sub-agent's output crosses a process boundary onto disk, and it is re-injected into a fresh parent turn at the next boot (which can fan out to a chat channel). An artifact that outlives the process cannot be gated on the run's attendedness, because the reader is a later process.
- Persistence is **opt-in**: until `init_and_reconcile` runs, every entry point is a zero-I/O no-op and the tracker behaves exactly as before. The boot orphan announcement (`init_and_announce_orphans`, the half that broadcasts) is `await`ed after `spawn_subagent_announce` (also now `async`) — a subscriber that is merely *scheduled* is not listening yet.
- **Boot reconcile claims two populations, not one (2026-09-02).** Tombstoning the orphans is one half; the other is every record that reached `Settled` with no `announced_boot` — a completion whose notice died with the previous daemon. The old `announced: bool` answered both "was it delivered" and "how many times have we tried", and it was written *before* the delivery, so a failed delivery had already stamped itself a success. It is now `announce_attempts: u8` (incremented on the way out, and the cap that stops an undeliverable record from being handed back forever) plus `announced_boot: Option<u64>`, written only after the broadcast returns.
- **The boot notice is one event per parent session carrying N children, and it counts instead of judging (2026-09-02).** `SubAgentCompletionEvent.success` is a single bool computed over the whole batch, so rendering it as a verdict told the model that every child shared one outcome and named one arbitrary child as the id to ask about — on a mixed batch (some finished, some interrupted) the finished children's results were discarded with it. The event now carries `request_ids: Vec<String>`, the whole batch (`#[serde(default)]`, so an event written before the field decodes as "no per-child list" and the announcer falls back to `request_id` — a missing key must read as "no list", never as "unreadable event"). `request_id` is deliberately `None` on a grouped notice: announce dedup asks the live tracker whether an id was already consumed, and the tracker has never heard of a pre-restart id. The header states the count, the per-child verdicts stay in `summary` where the producer wrote them, and `on_delivered(&request_ids)` stamps every id the delivery actually reached rather than one of N — the N-1 it could not stamp came back at the next boot.

### Fan-out width (`[execution] max_concurrent_subagents`)

Sub-agents executing concurrently within one agent run, default 4, clamped to `[1, 64]` at the enforcement point (`agents::subagent_tool::clamp_max_concurrent_subagents`). `0` is not "disabled" — it is a semaphore no child can ever acquire, so every fan-out would park until its batch deadline and return a partial result with nothing attempted. The semaphore is **per instance** (per agent run), so a live `[execution]` patch binds on the next run rather than resizing a fan-out already in flight. Raising it also widens the synchronous batch's wave arithmetic: a `rows`-wide batch runs in `ceil(rows / permits)` waves and each child's wall-clock share is divided by that count, so the per-child timeout cap moves with this knob by construction.

### HarnessDeps inheritance (Stage 5a / Stage A, 2026-05-08)

Subagents inherit the following from their parent via `SpawnerBase`:

- `guardrails` (Stage 5a) — Input/Output/ToolCall checks
- `fallback_llm` (Stage A, 2026-05-08) — Stage 5b single-step fallback
- `stall_config`, `consecutive_failure_cap`, `turn_timeout` (Stage A) — P0 stability triple
- `trace_sink` (Stage A) — observability sink
- `context_budget_config` (2026-07-29) — the `[context_budget]` config, from which the
  spawner builds the child's **own** budget + compactor + preflight pipeline
- `cheap_summary_provider` (2026-08-04) — the `[generation] cheap_model` tier, so the
  child compacts on the flash-tier model the root agent already uses for the same job

The context triple deserves its own note, because it was missing for a long time and
failed loudly rather than gracefully. `context_budget`, `context_compactor` and
`preflight_pipeline` were hardcoded `None` in the spawner's `HarnessDeps` literal — the
only three fields there with no comment saying why — so a subagent ran with **no context
management at all**: `build_prompt` replays the entire child session log every turn,
nothing ever compacted it, and when the provider finally answered `prompt_too_long` the
reactive rescue found no compactor, marked itself exhausted, and killed the whole child
run (`TerminateReason::ReactiveCompactExhausted`). Read-heavy research subagents are
exactly the ones that hit that wall.

The **config** is what travels, never a built instance: `ContextBudget` carries per-run
tokenizer calibration and circuit-breaker counters, and `ContextCompactor` must summarise
through the *child's* provider — sharing either would cross-contaminate a parent and its
concurrently-running children. Construction is a single point
(`subagent_spawner::build_context_triple`) and is all-or-nothing, which is the gating
`HarnessDeps` documents: a compactor without a preflight pipeline would pay for LLM
summarisation where free structural pruning was available.

The **cheap tier is the exception to "the child's provider"**, and it was missed for the
same reason the triple itself was: `ContextCompactor` has two construction sites, and only
the root one (`runner_impl.rs`) called `.with_cheap_provider(...)`. A second construction
site inherits no tier it is not explicitly handed, so every subagent billed its compaction
to the main model — which a swarm multiplies by its fan-out, silently and only on the bill
(§2.19 ④). It is asserted by **routing**, not by builder invocation:
`compactor::summarizer_name()` names the provider the compactor would actually call.

Two sibling builders are deliberately **not** called for children, and
`build_context_triple`'s doc says why so nobody "completes the set" later:
`with_cache_carryover` (a 16-slot LRU that one-shot child sessions would evict the parent
out of) and `with_summary_reuse` (keyed on a session that ends with the child).

Per the P1 zero-override decision, subagents do not currently support per-agent overrides for these fields. `AgentDef` may be extended with `Option<T>` overrides in P4 if needed, with full backward compatibility.

The shared assembly path lives in `src/orchestrator/deps_builder.rs` (`build_fallback_llm`, `build_stability_triple`); both the main runner (`aleph-server` boot) and the subagent spawner consume the same builders so wiring stays consistent.

### Provider / model routing at spawn (round-7, 2026-07-25)

`model` (and each `proposer_models` / `batch_tasks[].model` entry) is stamped
onto the child's requests verbatim by `ModelOverrideProvider`. Which *provider*
serves it is decided by `agents/runtime.rs::resolve_spawn_route`, in this order:

1. `agent_def.provider_hint` names a configured provider → pin it (the chain
   still falls through globally). Model string passed through untouched.
2. The model is `provider/model` and the prefix names the **parent's own**
   provider (read via `serving_provider_hint()`, canonicalised through
   `model_catalog::canonical_provider_id`) → keep the parent provider, stamp the
   bare model id.
3. The prefix names **another configured** provider (matched against
   `ProviderChain::agent_overrides`, same canonicalisation — so a `[providers]
   kimi` entry serves `moonshot/…`) → run the child on that provider with the
   bare model id. This is how one fan-out reaches several vendors:
   `proposer_models: ["openai/gpt-5.2", "anthropic/claude-opus-5"]`.
4. Nothing matched → the parent's provider, model string **untouched**. This is
   deliberate: an OpenAI-compatible aggregator primary (OpenRouter and friends)
   expects the qualified id on the wire, and such a deployment has no separate
   `[providers] anthropic` entry to match in step 3.

A bare model id is byte-identical to the pre-round-7 behaviour. Bare-name vendor
inference is deliberately **not** done — it would divert an aggregator primary's
traffic to a direct vendor. Ties between two entries canonicalising to one
vendor resolve to the lexicographically smallest key, never HashMap order.

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

### Durable recovery of background sub-agents (round-11, 2026-08-08)

`BackgroundAgentTracker` is process memory by design — two `RwLock<HashMap>`s
plus a `Notify`. That is the right shape for a live registry and the wrong
shape for the only record of a finished sub-agent: a daemon restart erases
every background child this run ever spawned, **including the ones that
already returned**, and the `completed` table's 1h TTL does the same thing
without a restart. The model's only handle is the `request_id` the spawn
returned, so it asked about work that had succeeded and got
`"No background sub-agent found"`.

The data was never actually lost. `spawn` writes
`SubagentSpawned { child_id }` into the **parent** session's durable event log
before the child's first turn and `SubagentReturned { child_id, summary }`
after its last, and the child's full transcript lives in its own session. What
was missing was an address: the tracker keyed on `request_id` while
`ephemeral_for()` minted `child_id` from an unrelated UUID.

**The wire.** `request_id` now travels
`AgentRuntimeConfig.request_id` → `SpawnRequest.request_id` →
`ephemeral_for(agent_id, request_id)`, so the child session key is
`Ephemeral { agent_id, ephemeral_id: "sub-bg-<request_id>" }`
(`SUBAGENT_BG_CHILD_PREFIX` is shared with the recovery reader). No schema
change: `SessionKey`'s string form already round-trips. Only the **background**
spawn passes `Some` — foreground, batch and MoA-aggregator spawns deliver
inline and have no id that outlives the call, so they keep the historical bare
nonce, `sub-<uuid>`.

> The two prefixes must stay distinct. Uncorrelated children go through the
> same spawner and write the same durable events, so if they carried the
> background prefix, `recovery::enumerate` would read each one's nonce as an
> unrecoverable `request_id` and `subagent list` would fill with every
> foreground sub-agent the session ever ran, each labelled recoverable — the
> same "the directory lies" defect, pointing the other way. Pinned by
> `anonymous_foreground_children_are_not_enumerated`.

> The correlation must be structural. One turn can spawn several background
> children, so their `SubagentSpawned` events share a `turn_id` and cannot be
> told apart by position — the same parallel-batch ambiguity that made
> `tools::scoped::dispatch` replace a session-log scan with an ambient call
> identity. Changing the shape minted by `ephemeral_for` silently blinds
> recovery; `recovery.rs::child_key_roundtrips_through_the_request_id` is the
> test that goes red first.

**The reader** (`src/agents/subagent_tool/recovery.rs`) is lazy: it runs only
when the tracker reports an id it has never seen, and one `get_events` serves
every unknown id in that call. **Four** verdicts, not three — the sidecar
(`background_persistence`) is a second durable source covering a set the event
log cannot see, and it earns its own arm:

1. `SubagentReturned` present → `completed_recovered` carrying the real summary;
2. only `SubagentSpawned` → `interrupted`, plus a `child_session` pointer, the
   progress counters, the named in-flight calls and the transcript tail;
3. a sidecar record → its **outcome's** word (`completed` / `failed` /
   `timed_out` / `cancelled` / `interrupted_by_restart` / `settled_unknown`,
   from `background_persistence::settled_label`), merged with whatever the child
   log adds rather than replacing it — only `completed` carries the "do NOT
   re-run it" seal;
4. neither source → the pre-existing `unknown`.

Wired into `check_status`, `wait` (single and `wait_any`), `cancel`,
`wait_cancelled` and `list`. The detail faces pay one child-log read per
unfinished row; the directory face (`list`) does not, and renders
`progress: null` — "did not ask", never "no progress".

Every verdict is `ToolResult::Success`, including `interrupted`: a restart is
not a verdict on the call the model is making now.

`list` gained a `from_durable_log` array. It documents itself as the way to
"recover a request_id you no longer hold", so a directory that reports an empty
session after a restart is the directory lying. That costs one read per `list`
call; `list` is an on-demand directory, and cheap-and-wrong is not the trade to
make there.

Those rows go through `recovery::to_list_row`, not `to_json`: an entry reaches
this path **permanently** once the tracker's TTL prunes it, so carrying each
finished sub-agent's whole output would make the directory grow with session
age. Rows preview at `LIST_RESULT_PREVIEW_CHARS`, state `result_chars` so the
preview cannot read as the whole thing, and cap at `MAX_LISTED_COMPLETED` while
naming how many were withheld — the same anti-silent-truncation rule the live
half follows. `check_status` on the id still returns every byte.

`annotate_unknown`'s wording was fixed in the same pass. It used to tell the
model that every unknown id "will never complete — drop them from your next
wait". After a restart *every* id is unknown to the tracker, so following that
advice throws away finished output and redoes the work.

`BackgroundAgentTracker` stays purely in-memory; the durable lookup lives in
the tool layer, which already holds `session: Arc<dyn SessionService>` and
`parent_session_id`.

**Recovery reports; it does not restart.** Whether to re-run an interrupted
child, and from where, is the model's call (R7). Auto-resuming one would mean
rebuilding its cwd, tool permissions and parent-run liveness, and would burn
tokens unattended.

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
    TaskCompleted,
    TaskFailed,
    SessionStarted,
    SessionConcluded,
    SessionDeadlocked,
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

Five variants with zero producers and zero stored-string consumers
(`TaskCreated`, `ArtifactSubmitted`, `DigestGenerated`, `ShutdownRequested`,
`ShutdownResolved`) were cut on 2026-07-24; `read_event_row` skips unknown
stored strings, so legacy rows need no migration.

**Retention**: Active teams — events older than 72h pruned lazily (during `team_digest` generation or periodic cleanup). Disbanded teams — events older than 24h pruned. Used for `team_digest` generation.

### Group Chat (`teams.chat.*`)

A user message to a team fans out through the `GroupChatBroadcaster`
(`src/teams/broadcast/mod.rs`): mentioned members (or `@all`) run concurrently,
their replies can chain further @-mentions, and three storm gates cap the tree
(chain depth, per-round fan-out width, cumulative activations — operator-tunable
via `[team_broadcast]`). Lifecycle ownership (2026-07-24):

- **Fan-out tree identity**: the `run_id` returned by `teams.chat.send` names
  the whole fan-out tree. It is registered (`register_fanout`) in the
  `BackgroundAgentTracker` *before* the dispatch task is spawned, so the id is
  cancellable the instant the RPC returns; each member run registers under it
  (`SpawnMeta.parent_id` = tree run_id) with a child token, so cancelling one
  member never poisons the tree.
- **`teams.chat.cancel`**: poison-then-walk — fires the tree's cancellation
  token first (no new member spawns at any recursion level), then walks
  `running_children_of(run_id)` and aborts each in-flight member through the
  engine's per-run cancel (`ExecutionAdapter::cancel`, the same wire
  `agent.cancel_run` uses). Residual race: members between the last-instant
  poison check and engine run registration escape — bounded by one level's
  fan-out width, each capped by the member timeout below.
- **Member run timeout**: each member run carries
  `member_run_timeout_secs` (default 600 s, `[team_broadcast]` knob;
  previously `None` → the engine's 48 h fallback). A failed or timed-out
  member posts a system line to the team transcript ("@X 的发言执行失败或超时")
  instead of vanishing silently. Note the leader's group-chat turn is a member
  run too — synchronous `team_delegate` calls must fit inside the window
  (the delegate side's `SettleOnDrop` fence marks the coord task Failed if the
  awaiting run is dropped mid-delegation).

This is the teams peer group chat — distinct from the `src/group_chat/`
persona roundtable (`[group_chat]` config), a separate system.

### Who is acting in a team tool call (2026-08-08)

Every team / collaboration tool resolves its actor **per call**, through
`builtin_tools::acting_agent::acting_agent_id(&self.current_agent_id)`, which
reads the `TURN_CONTEXT` task-local that `ScopedToolService::execute` — the
single production tool-dispatch chokepoint — scopes around every tool call.

This replaced a constructor argument resolved once when `BuiltinToolRegistry`
was built. `BuiltinToolConfig::current_agent_id` had **no producer anywhere in
the tree**, so the `unwrap_or_else(|| "main".to_string())` fallback fired every
time and the literal `"main"` was welded into ~20 tools for the process
lifetime. The consequences were silent rather than loud, because the wrong
identity is a perfectly valid identity:

- `team_member_add` compared `team.leader_id != "main"` no matter which agent
  was running, so a team led by `researcher` **refused its own leader** and
  accepted `main`;
- `workflow_step_review` filed every approval under `main`;
- `inbox_read` read `main`'s inbox from inside a worker's turn.

The constructor argument survives as a **fallback** for call sites outside a
turn scope (direct construction in tests, RPC faces, background paths). It is no
longer the answer. Two properties to preserve when touching this:

- **The empty-string filter is reachable, not defensive padding.** Most keys are
  built via `SessionKey::main`, which normalizes `""` to `"main"`, but
  `routing::resolve::build_session_key` fills `agent_id` straight from its
  `&str` argument with no normalization — and `plan_submit`, `plan_resolve` and
  `lifecycle_resolve_shutdown` read an empty actor as "unknown, fall back to the
  team leader". An empty turn identity must not overwrite a good constructor
  value with nothing.
- **It returns the BASE agent id** (`main`), not the project/user-scoped
  composite (`main__u-alice`). Team rosters and task ownership are keyed on the
  base id — a roster stores `researcher`, never `researcher__u-bob` — so making
  this scoped would break every leader comparison in the team layer. A caller
  needing the scoped form composes it explicitly, the way the `remember`
  dispatch arm does.

### Member Tool Surface (成员工具面)

A team member created inline may declare the tools it is allowed to call.
Declared on `CreateAgentSpec` (`team_create`) and on `TemplateMember` /
`TemplateLeader` (team templates) as `tools` / `tools_denied`; both accept a
trailing-`*` prefix glob (`task_*`). **Omitting them keeps every tool**, which
is what every team got before the fields existed — so no existing team or
template changes behaviour.

The declaration lands in `teams::member_provision`, the shared provisioning
tail both creation paths call. It must reach **both** ends of one chain:

```text
AgentDefinition.skills ──from_resolved──▶ AgentInstanceConfig.tool_whitelist
                                                     │
                                          AgentInstance::is_tool_allowed
                                                     │
                    run_loop/inner.rs:156 ── retain ──▶ tools the model sees
```

The runtime config governs the current boot; the persisted `AgentDefinition`
is what `AgentInstanceConfig::from_resolved` rebuilds from after a restart.
Writing only one silently widens or narrows the surface on restart — hence the
single tail (two copies were two chances to write one end) and the
`restart_round_trip_preserves_the_surface` test.

**Contract validation (fail-fast).** A member's launch prompt *instructs* it to
call specific verbs: `broadcast::member_prompt` tells workers to hand work back
with `task_submit`; `teams::leader_prompt` gives leaders the four-step
`task_create` → `team_delegate` → `task_read_artifact` → `task_review` duty.
A declared list that hides these does not yield a narrower member — it yields
one told to do something it cannot do, failing silently at the first hand-off.
Creation therefore **rejects** such a declaration, naming the missing tools,
before any directory is created. Essentials live in
`WORKER_ESSENTIAL_TOOLS` / `LEADER_ESSENTIAL_TOOLS`.

Note the glob matcher is `gateway::agent_instance::tool_allowed_by`, shared
with the run-time gate — validation and enforcement cannot disagree.

**Not exhaustive, and not a security boundary.** `get_tool_schema` and
`subagent` are registered into the loop registry *after* the allowlist filter
runs (`run_loop/inner.rs` — the "collapsed-but-unsnapshotted" class), so a
declared list never removes them. Runtime QA confirms it: a member declaring
`["task_*", "message_send", "search", "web_fetch"]` enumerates exactly those
plus those two. Since `subagent` spawns a differently-scoped agent, treat
`tools` as attention/accident scoping — the enforcement boundary stays
`src/tools/scoped/` + exec tier + the sandbox floor.

**Not derived from `role`.** `role` is free text ("估值建模"); inferring a tool
surface from it would be keyword matching, which P8 forbids. Declaration is
explicit or absent.

**Built-in templates: two declare, two do not.** `strategy-room` (moderator +
bull/bear/contrarian) and `code-review` (lead-reviewer + four lens reviewers)
declare a surface; `software-dev` and `research-paper` deliberately do not.
Two reasons line up. Fit: the build/run roles genuinely need a broad dev
surface, and §"Not exhaustive" means guessing narrow is unrecoverable — a
`tools` list is a `retain`, so an excluded tool cannot be promoted back with
`tool_search` the way a mode-deferred one can. Blast radius: template member
ids are **global agent ids**, and the generic ones (`lead`, `backend`,
`frontend`, `qa`, `pi`, `reviewer`, `analyst`, `writer`) all live in the two
undeclared templates.

`team_*` is never globbed in a declaration — the family contains
`team_disband` / `team_create` / `team_from_template` / `team_member_remove`.
Enumerate `team_status` and `team_delegate`. `task_*` is safe to glob.

For code-review the surface keeps `bash` (reading a diff needs `git diff`) and
drops `file_write` / `file_edit` / `file_ops` / `apply_patch`. Since bash can
write, that is attention scoping — it targets the reviewer who "helpfully"
edits the code it was asked to review, not an attacker.

**When a declaration is dropped.** `provision_member` reuses an existing agent
by id and skips `tools` entirely (an existing agent keeps its own surface), and
a `self` leader is the caller's own agent. Both cases are reported:
`MaterializedTeam.tools_ignored_for` → `TeamFromTemplateOutput.tools_ignored_for`,
omitted from the output when empty. Guards live in `templates/materialize.rs`
tests: every built-in role satisfies its contract, only the two reasoning
templates declare, no reviewer carries an edit tool, nothing globs `team_*`.
The report does not yet reach every caller: `interfaces/webchat/src/api/teams.rs`
extracts only `team_id` from the `team_from_template` tool output and discards
the rest, so a Panel user creating a team from the "create from template"
dialog does not see `tools_ignored_for` — only the LLM tool-call path does.

#### Live surface: the `team.<id>.*` topic family

Everything the Panel's group-chat view knows in real time arrives on five
topics, all sharing one `{topic, data}` envelope published through the single
source `gateway::event_emitter::team_fanout::publish_team_event` (the
per-run `TeamFanoutEmitter` calls it too):

| Topic | Payload | Published by | Panel effect |
|-------|---------|--------------|--------------|
| `team.<id>.message` | `{agent_id, text, run_id, final}` | `TeamFanoutEmitter` on `RunComplete` | attributed bubble |
| `team.<id>.system` | `{text}` | `GroupChatBroadcaster::post_system` | centered notice chip |
| `team.<id>.activity` | `{agent_id, status: working\|done\|error}` | member spawn, `ToolStart`, `RunComplete`, `RunError`, adapter failure | roster status dot |
| `team.<id>.fanout` | `{run_id, status: started\|settled}` | `dispatch_user`, head and tail | `active_run_id` + `ChatPhase` |
| `team.<id>.task.<verb>` | `{task_id, status, …}` | `CoordTaskStore` | task strip / drawer |

Contract notes (2026-07-25):

- **One parse point.** The Panel resolves the topic exactly once, in
  `views::chat::team_events::parse_team_topic`, which returns `(team_id, kind)`.
  Team ids are opaque and may contain dots, so the kind is matched as a suffix,
  not by positional split; `team.changed` (the global team-list invalidation)
  deliberately does not match. Any new consumer must go through it rather than
  re-testing `topic.starts_with("team.")` — that shortcut is what leaked a
  background team's bubbles into whatever conversation happened to be open.
- **Scope before projecting.** Gateway subscription is the `team.*` wildcard, so
  every team the daemon runs is delivered. Consumers filter on the team the user
  is actually viewing. The sidebar uses the same parse to badge *other* groups as
  unread instead of dropping their activity on the floor.
- **`fanout` owns the run slot.** `started`/`settled` is the only writer of
  `active_run_id` in team mode, which is what gives group chat a Stop button
  (routed to `teams.chat.cancel`, not `chat.abort` — a fan-out tree is not an
  engine `active_runs` entry) and, for free, the busy→idle edge the composer's
  prompt-queue auto-drain already watches.
- **`system` is not an agent.** Storm-guard explanations used to be
  transcript-only: the gates fired, the room went quiet, and a live user saw
  nothing until they left and came back. They now go out live and render as
  chrome, never as a bubble from a participant named `system`
  (`broadcast::SYSTEM_HANDLE`).

#### History replay fidelity

`teams.chat.history` replays the durable transcript, but `team_messages` is a
shared bus that also carries **directed inbox traffic** (the notifier's leader
digests, thread-escalation hints, discovery pings). `map_history` keeps only
conversation rows — `MessageType::Message`, or any recipient-less row — and
stamps each with a server-derived `kind` (`user` | `agent` | `system`) so the
Panel renders replayed history identically to what it showed live. The row cap
is applied *after* filtering (raw fetch over-reads 3×), so a burst of
notifications cannot squeeze the conversation out of the window. Panels talking
to an older core default `kind` to `"agent"` and fall back to the reserved-handle
check for the user's own rows. (teams: rewire the group-chat Panel surface (§4.5))

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
  (`advisors: Vec<MoaSlot>` + `aggregator: MoaSlot`), `fanout` cadence,
  `advisor_timeout_secs`, `advisor_max_tokens`/temperatures,
  `default_preset`, `save_traces`.
- **Fan-out cadence** (`fanout`, one wire string) — how often advisors are
  re-consulted within a run. Advisor spend and latency multiply by this, so it
  is the subsystem's main cost lever:
  - `per_iteration` (default) — re-consult whenever the advisory view changes,
    i.e. every tool iteration. Most informed, most expensive.
  - `user_turn` — consult once per run and reuse that advice. Cheapest; the
    original MoA shape.
  - `every_n:<N>` (N >= 2) — consult on the first state advance, then every
    Nth; the iterations between reuse the last advice, so the aggregator is
    never advice-less, just not refreshed against the very latest tool result.
  A turn whose advisory view is byte-identical to the previous one is the
  harness re-issuing a request, not the task advancing: it always reuses and
  never consumes a cadence slot (`FanoutState.last_seen_signature`).
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
- **Side channels** (round 9): `MoaProvider` is the shape of the *user's
  turn*, and only the Think loop drives it. Anything else in the run that
  reaches for "the run's provider" — today that is history summarization via
  `ContextCompactor` — takes `MoaProvider::acting_chain()` instead: the same
  acting model, without the fan-out. Going through the facade made every
  compaction pay N advisor calls, appended "use the advisor responses below"
  to the *summarizer's* prompt (so advisory framing could reach the persisted
  `<session_context>`), and consumed a cadence slot while overwriting the
  run's cached advice — which the next real turn then reused. `runner_impl.rs`
  derives `side_channel_llm` for this, wrapped in the same per-agent
  `MeteringProvider` so side-channel spend is still attributed.
- **Accounting**: advisor usage is metered per-slot (`MeteringProvider`
  labelled `moa:<idx>:<provider>:<model>`) and kept OUT of
  `ProviderResponse.usage` so the context gauge stays honest; a summed
  `MoaAdvisorSpend` event restores visibility. Four `LoopTraceEvent`
  variants — `MoaAdvisor`, `MoaAggregating`, `MoaAdvisorSpend` (live) and
  `MoaTurnTrace` (persist-only, gated by `save_traces`) — are the harness's
  only touchpoint (`src/harness/trace.rs`). The panel renders the three live
  events inline as reasoning blocks (◇ 顾问 / ◆ 聚合 / ▫ 开销,
  `interfaces/webchat/src/platform/wide/views/chat/events.rs`). Since round 9
  each `MoaAdvisor` fires the moment that advisor lands (a `FuturesUnordered`
  over the fan-out), not in one batch after the slowest returns — advisors
  differ wildly in latency and the batch shape left every surface dark for up
  to `advisor_timeout_secs` on each tool iteration. The event still carries its
  own SLOT index, so what a user reads never depends on who answered first.

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
- **Advisory-view sizing**: one view is built per turn and shared by the whole
  fan-out, so it is budgeted against the SMALLEST advisor context window
  (`advisory_view.rs::view_budget_chars` over
  `model_catalog::resolve_context_window`), converted to characters through
  the content-aware `pressure::chars_for_token_budget` — the same token budget
  buys ~3.5x more characters of English prose than of CJK, so a flat character
  limit over-allocates a Chinese conversation into the exact 4xx it exists to
  prevent. Over-budget messages SHRINK head+tail rather than disappear, so the
  message count, order and role sequence are invariant and "first message must
  be user" cannot break. `ADVISORY_VIEW_BUDGET` remains as an upper bound.
- **Degraded fan-out**: an advisor that fails, times out, is skipped by the
  breaker, or answers empty is carried as a non-advising `AdvisorOutcome`
  (`advised: false`, set at construction — never re-derived by sniffing the
  text for a `[failed:` prefix). Those slots stay visible to the aggregator in
  the guidance roster, with their reason, but are not numbered as responses to
  read; when NO slot advised, the "use the advisor responses below" framing is
  dropped entirely. Provider error text riding into the prompt is clamped
  (`ADVISOR_NOTE_BUDGET`); the trace/panel copy (`AdvisorResult.error`) keeps
  the full message.
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

**Round 6 — advisor-side resilience** (2026-07-25, spec
`docs/superpowers/specs/2026-07-25-moa-round6-advisor-resilience-design.md`).
Rounds 1–5 hardened activation, configuration and observability; round 6 is
the first pass over what happens to the fan-out when an *advisor*
misbehaves or the conversation gets long. No new config, RPCs or trace
events.

- **Run-scoped advisor circuit breaker**
  (`src/providers/moa/advisor_health.rs`). The fan-out had no memory: under
  `per_iteration`, a hung or mis-keyed advisor burned the full
  `advisor_timeout_secs` (default 120 s) on **every** tool iteration — up to
  40 minutes of dead wall-clock on a 20-step run, each wait blocking the
  aggregator. Two consecutive failures now retire a slot for the rest of the
  run (a recognised *permanent* error — auth failed, model not found —
  retires on the first strike); a success resets the count, and the next run
  starts fresh, so recovery is automatic. It reuses
  `providers::health`'s `ProviderError` + `From<&AlephError>` classifier —
  which is now the *only* thing that module carries, and this is one of the two
  consumers keeping it alive. It deliberately never reused the `ProviderHealth`
  state machine that used to sit beside it: that machine's `Degraded` cooldown
  suppresses the very retry a strike counter needs, so a dead advisor would
  never trip — it would settle into probe / cool-down / probe, re-paying the
  timeout every backoff window. `ProviderHealth` has since been deleted
  outright (see FEATURE_LOCATOR §3.6 round-3: it gated no dial), so the
  distinction is now structural rather than a rule to remember. Round-1's "no K-of-N racing" decision is
  untouched: that governs how one consultation completes, this governs
  whether a slot already proven dead this run is consulted at all.
- **Retired slots keep their slot**: they render as
  `Advisor N — <label>: [skipped: <reason>]` in the guidance, structurally
  like the existing `[failed: …]` / `[timeout after Ns]` notes. The
  aggregator can therefore tell "one advisor configured" from "three
  configured, two down", and advisor numbering never shifts mid-run. Two
  counts are deliberately different and must stay so:
  `MoaAggregating.advisor_count` / the `i/n` display is the TOTAL slot count;
  `MoaAdvisorSpend.advisor_count` counts only slots actually CONSULTED.
- **Advisors see the acting agent's tool roster**
  (`prompts::advisor_system_prompt`). The advisor prompt asks for "tool-use
  strategy", but the advisory view only reveals a tool once it has already
  been called (`[called tool: X]`) — so advisors invented names for
  everything else. `payload.tools` was already owned by
  `MoaProvider::process` and handed to the aggregator alone; it now also
  renders a budget-capped roster (`name: first sentence`, 100 chars/line,
  1800 chars total, `+N more tools` tail) appended to the advisor system
  prompt, with the "you still cannot call any of them" framing intact. Zero
  tools ⇒ the prompt is byte-identical to before. hermes references are
  blind here too, so this is a surpass item.
- **Whole-view budget** (`advisory_view::apply_view_budget`). Per-tool-result
  truncation bounded one result; nothing bounded the view. The failure that
  matters is not cost (providers auto-cache prefixes, and
  `mark_cache_breakpoints` already covers Anthropic) but **overflowing the
  advisor's context window** — an advisor on a smaller-context model than the
  aggregator dies with a hard 4xx on every later iteration of a long run.
  The view is now clamped to `ADVISORY_VIEW_BUDGET` chars by *shrinking* the
  oldest messages, never dropping them, so message count, order and role
  sequence are untouched and the "first message must be `user`" rule a
  drop-based elision would have to re-establish simply cannot break. The
  newest message is exempt from the stub allowance. Runs before
  `view_signature`, so the cache key describes what is actually sent.
- **Empty-turn guard**: `build_advisory_view`'s `User` arm pushed
  unconditionally while the `Assistant` arm guarded — a blank user turn
  reached the wire as `MessageContent::Text { content: "" }` (anthropic's
  `blocks.is_empty()` fallback only catches a wholly empty content vec) and
  400'd, failing every advisor at once. Blank user turns are now skipped and
  the view is guaranteed non-empty. The dead `last_user_text` refill this
  exposed (unreachable: rendering nothing implies the stash is `None`) was
  removed.

### One-Shot Task Fan-Out (existing, previously undocumented)

Independent of the port above, the `subagent` tool has always supported a
Mixture-of-Agents shorthand: `proposer_models` (replicate the top-level
`task` across models as parallel proposers) + `synthesize` (run ONE
aggregator sub-agent that folds the proposals into a single answer) +
`aggregator_model` (`src/agents/subagent_tool/` — see `parse.rs` /
`loop_tool.rs`; Wang et al., "Mixture-of-Agents Enhances Large Language Model
Capabilities", 2406.04692). `synthesize` requires a foreground batch
(`run_in_background=false`) and returns `status: "moa_completed"` with a
`synthesis` field plus the raw `results` — or `moa_synthesis_failed` if the
aggregator itself failed, or `moa_no_proposals` if no proposer survived, so a
requested reduce that never ran is never reported as a plain `batch_completed`.

Each `proposer_models` entry follows the same rules as `model`, so qualifying
them (`["openai/gpt-5.2", "anthropic/claude-opus-5"]`) fans the proposals out
across *vendors* rather than across models of one provider — see "Provider /
model routing at spawn" above. Unqualified names all run on whichever provider
the parent holds.

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

### Scheduled work runs on somebody's behalf (cron / heartbeat)

The root `CLAUDE.md` routes `src/tasks/cron/` and `src/tasks/heartbeat/` here,
so the one thing an editor of those two must not re-derive belongs here too —
**pointing at the owner rather than restating it**, because the full account
lives in [FEATURE_LOCATOR §5.22](FEATURE_LOCATOR.md) round-10 ⑬⑭⑮ and in
[SECURITY.md](SECURITY.md)'s deactivation-freeze paragraph.

- **Both carry `(owner_user_id, scope_id)`, stamped at creation by ONE
  derivation.** `CronJob::stamp_current_scope()` and its heartbeat twin are
  called from every production construction site — RPC face, tool face, and the
  governance-job builders — and a source-level census fails by name when a new
  construction site appears without one. Before 2026-09-03 the cron RPC face
  stamped neither column and `HeartbeatTask` had neither column at all, and the
  consequence was never an error: four readers short-circuit on NULL (the
  deactivation freeze treats the job as owned by nobody, the fire-time
  `walled_owner_reason` check reads it as legacy, the run executes with no
  scope, and the spend lands on `@unattributed`). That census is a NAME census,
  not a dataflow proof — a body that stamps a different job than the one it
  constructs passes — and its own doc says so.
- **Heartbeat is the freeze's fourth leg, and the twin is still open.** A
  deactivated principal's monitors are disabled by
  `tasks::heartbeat::service::ops::pause_all_owned_by`, alongside goals, loops
  and crons. Cron additionally has a **fire-time** backstop
  (`walled_owner_reason` → `disable_walled_owner_job`) that catches a job
  re-enabled by a second admin after the sweep; **heartbeat has no counterpart**,
  which is a recorded debt, not an oversight.

### Outcomes that are not verdicts (2026-08-08)

`MemberRunStatus` has three non-failure outcomes, and the distinction is a
budget question: `budget_failures_since` counts `Failed` and `Timeout`, not
`Abandoned`.

| outcome | maps to | why it must not spend a retry |
|---|---|---|
| `Busy` | `Abandoned` | the target already had a run; **this attempt never started**, and an attempt that never started is not a failed attempt. Newly reachable since team runs began stamping `busy_input_mode = "queue"` — before that a same-session collision was folded inline and the turn's intent was lost |
| `Cancelled` | `Abandoned` | an operator `run.cancel`, the session cancel sweep, or a parent tearing its children down. Nothing about the task was judged; the attempt was interrupted |
| `Completed` / `Failed` / `Timeout` | themselves | actual verdicts |

Classification lives in one free function rather than inline `match` arms in
`execute_member_task`, so it is testable without standing up a run.

**Crash recovery has a ceiling.** `Abandoned` runs correctly skip the retry
budget, but `reclaim_orphaned` re-stamps `started_at` on every re-dispatch, so
`zombie_ttl_secs` could never watch the age accumulate and a task that reliably
killed its worker was re-dispatched **without any bound at all**. Bounded by
`MAX_TASK_RECOVERIES` (2 free recoveries, terminal on the third crash), counted
by `recovery_abandons_since`, which shares the `retry_budget_reset_at` anchor so
an operator hard-retry re-arms this ceiling exactly as it re-arms the retry
ladder. It is a constant rather than a `DispatcherConfig` field because the boot
site builds that struct with an exhaustive literal in the `aleph-server` bin
crate — see FEATURE_LOCATOR 附录 A #14 before promoting it.

**Cancel is a dispatcher janitor, not a per-tool adapter.** Cancelling a task
used to write the task row and leave the member run burning for up to 24 h. The
sweep is mechanical and lives in the dispatcher, so every present *and future*
cancel surface inherits the effect rather than each tool growing its own
adapter.

**A paused workflow step stays paused across a restart.** Pause used to count
in-flight steps instead of persisting them, so a restart quietly un-paused them.
The pause marker is durable and provably cleared on resume — it has to be,
because a paused row is invisible to every janitor.

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
| `subagent` (action=`run`) | Spawn an ephemeral sub-agent for a focused task (`batch_tasks` = parallel fan-out; `run_in_background` = fire-and-forget) |
| `subagent` (action=`check_status`/`wait`) | Poll or event-park on a background sub-agent |
| `subagent` (action=`cancel`/`list`) | Fire a run's CancellationToken / enumerate running+completed (presence-only fan-out entries excluded — see "Argument validation" / `list` Action) |
| `subagent` (action=`send_message`/`read_inbox`) | Team-store messaging faces (`team_name` → team id via `TeammateManager::ensure_team`) |

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
  may `wait` again. The timeout report carries the same `summary` + `progress`
  pair `check_status` gives, so deciding "keep waiting or cancel?" does not cost
  another turn for data the tracker already had.
- `{"action": "wait", "request_ids": ["a", "b", ...], "timeout_secs": 120}` —
  fan-out first-completion: returns as soon as any id in the set finishes that
  has **not already been returned to you**, reporting which one (`request_id`).
  Drain the set by **re-issuing the same call**; once every id has been handed
  over and none is still running you get `{"status": "all_delivered",
  "request_ids": [...]}`. A failed first-completion comes back as a
  `status: "failed"` Success (not a `ToolResult::Error`) so it does not trip the
  harness failure counter — the model sees which child failed and can wait for
  the rest. Ids the tracker has never heard of come back as
  `unknown_request_ids` instead of being silently parked on. Mirrors codex
  `wait_agent`.

`timeout_secs` defaults to 120 and is clamped to 600 (`DEFAULT_WAIT_TIMEOUT_SECS`
/ `MAX_WAIT_TIMEOUT_SECS` in `types.rs`) — well under the subagent tool's
`1_800_000`ms wall-clock budget, so a single `wait` can never hang the turn.

**A parked wait is interruptible.** Both branches run inside a
`tokio::select!` against the harness's per-call `CancellationToken`, so a
`/stop` (or a per-tool cancel) is honoured immediately instead of waiting out
the window — that window is up to ten minutes, and the cancel token's worst-case
latency is exactly the longest sleep on the path. codex's `wait_agent` treats
new input as a first-class wake reason (`WaitOutcome::Steered`) for the same
reason. The interrupted wait reports
`{"status": "wait_interrupted", "still_running": [...]}` as a **Success**:
nothing failed, and the sub-agents themselves are untouched by this token (a
run-level cancel reaches them through their own). Reporting it as an error would
feed the harness's consecutive-failure counter and the cross-batch memo a verdict
about a call that was merely interrupted — the trap `ToolError::Cancelled` exists
to avoid one layer down.

**Delivered results are skipped, not re-returned.** `wait_any` satisfies the wait
only from an *undelivered* completion and marks it consumed on the way out. This
is what makes "re-issue the same `request_ids`" a correct drain loop. Returning
the first completed id regardless of delivery meant a model repeating its own
previous arguments got the same result back instantly and forever, burning one
LLM turn per lap. A single-id `wait` stays idempotent: the caller asked about one
agent, so `wait` maps the `AllDelivered` terminal back to that agent's snapshot.

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

`mark_consumed` works in **both** phases of a run's life, because its two
producers sit on opposite sides of the completion transition. A `wait` /
`check_status` stamps an existing completed entry. A model-issued **`cancel`**
cannot: it fires the token and returns long before the child unwinds, so at that
moment there is no completed entry — the intent is recorded on the running entry
(`RunningAgent.consume_on_completion`) and folded in by `mark_completed`, so the
result is *born* consumed. Covering only the first case left cancellation as the
likeliest producer of a spurious announce: a whole parent turn spent reporting
`sub-agent failed: cancelled` for a child the parent itself killed.

The dedup guard is also re-checked **before every retry attempt**, not once up
front. The announce retry schedule (`[0, 30, 120]s`) spans over two minutes, and
the commonest reason the parent is busy is that it is parked in this very `wait`
— which returns the result and consumes it. Checking only before the loop meant
the announce woke minutes later and spent a fresh turn re-delivering something
the model had already folded in.

### list Action

`{"action": "list"}` is a **directory** of the background sub-agents this
session still holds — running and recently-finished — so the parent can recover
a `request_id` it no longer has in context:

```json
{
  "running":   [ { "request_id": "...", "task": "...", "elapsed_secs": 12 } ],
  "running_count": 1,
  "completed": [ { "status": "completed", "request_id": "...", "task": "...",
                   "result_preview": "first 200 chars…", "result_chars": 8123,
                   "iterations": 4, "duration_secs": 31, ... } ],
  "completed_count": 1,
  "note": "Rows are summaries. Use 'check_status' ... for full result text."
}
```

**Rows are summaries, and the list is scoped to this session.** Two properties
that are easy to lose and expensive to lose:

- *Scope.* The tracker is a **process-global** singleton. `flat_nodes` has always
  taken a `root_session` filter; `list_running` / `all_completed` did not, so an
  unscoped `list` handed one session's model the live `request_id`s of **every
  other session's** background sub-agents — ids it could then `check_status`
  (reading foreign output) or `cancel` (stopping foreign work). Both now take
  `scope: Option<&str>` and the tool passes its `parent_session_id`; `None` is
  reserved for callers that genuinely have no owning session (CLI / direct
  construction / tests). A model's ids can only come from its own spawns or from
  an enumeration face, so scoping the enumeration is sufficient — by-id lookups
  keep their semantics.
- *Size.* `list` used to render every retained completion with its **full**
  result text (up to `MAX_COMPLETED_RESULTS` = 256 of them), which one call could
  use to swamp the parent's whole context with material it never asked for — and
  which the generic result budget would then chop from the middle. Rows now carry
  a bounded `result_preview` plus `result_chars`; a failure message rides in full
  because it is short and is the point of the row. The list is capped at
  `MAX_LISTED_COMPLETED` = 20, ordered newest-first by `all_completed` (the
  backing map iterates randomly, so an unordered cap would drop an arbitrary
  subset), and when it truncates it says how many were withheld and how to reach
  them — never a silent cut.

`list` reports only ids the parent can act on. *Presence-only* registrations —
the running-only entries the sync fan-out seams create (sync `batch_tasks` /
MoA proposals and aggregator, `team_delegate`, team-chat members, the fan-out
tree root) — are excluded, because they deliver their result inline at their own
seam and never reach `completed`: listing them handed the model request_ids whose
every follow-up `check_status` / `wait` answers "no background sub-agent found".
They remain fully visible to the cancel walks (`session_has_running`,
`running_runs_of_session`, `running_children_of`) and to `cancel(id)`. The same
exclusion applies to `flat_nodes` / the `subagent.tree` RPC — that snapshot cold-
starts a live event stream, and nothing emits `Spawned` / `Settled` for these
entries, so including them left Panel rows frozen at `Running`. Re-admitting them
to the tree requires giving `RunningRegistration::drop` an honest terminal
lifecycle (it cannot observe the outcome today) plus the paired events; simply
dropping the filter reinstates the ghost rows.

### Argument validation (round-7, 2026-07-25)

The `subagent` tool's arguments are parsed by hand (`subagent_tool/parse.rs`),
not by serde, so two guards stand in for `#[serde(deny_unknown_fields)]`:

- **Unknown top-level keys are rejected**, with the accepted set in the error. A
  near-miss (`agent` for `agent_type`, `prompt` for `task`, `background` for
  `run_in_background`) previously ran with a different meaning than requested and
  reported success. An explicit JSON `null` counts as absent, since
  schema-completing providers emit `"key": null` for properties they are not
  using. The accepted set lives in `types.rs::ACCEPTED_ARG_KEYS` and a drift-guard
  test pins it bidirectionally to the advertised schema properties (sole
  exception: `name`, kept out of the schema so its dedicated "sub-agents are not
  addressable teammates" rejection can fire).
- **`timeout_secs` is clamped** into `[1, budget − headroom]` for `run` and for
  every `batch_tasks` entry. The ceiling is derived from the `subagent` row in
  `tools::budget`, so the child's own wall-clock timeout always fires before the
  tool budget — the model then gets an actionable `Sub-agent timed out after Ns`
  instead of an opaque budget overrun that discards a child still doing work.
  `0` no longer means "time out before the first turn".

### Why cap=50?

This is a designed memory/observability tradeoff (P2 Q6, hardcoded). For
long-running background subagents (>50 tool calls), only the most recent 50
steps remain visible. Configurable cap is a future stage if needed.

## Named Tool Sets (P2 Stage G)

`AgentDef.allowed_tool_sets: Vec<String>` lets agent definitions reference named
tool collections instead of (or alongside) flat allowlists. Three sets are
predefined:

| Name           | Tools                                                       | Purpose                                       |
|----------------|-------------------------------------------------------------|-----------------------------------------------|
| `READ_ONLY`    | file_read, file_ops                                         | Pure filesystem inspection                    |
| `INVESTIGATION`| file_read, file_ops, search, web_fetch, subagent            | Read-only research with remote sources        |
| `ASYNC_SAFE`   | file_read, file_ops, search                                 | Background-safe (no side effects, no exfil)   |

> Doc correction (2026-07-25): this table listed `glob, grep, read_file` — names
> no registered tool ever bore. The sets themselves were fixed in `tool_sets.rs`
> (whose header records the same phantom-name incident: an INVESTIGATION-mode
> agent saw 1–2 usable tools and gave up after one turn); the table had kept the
> pre-fix values. The canonical builtins are `file_read` (single-file read) and
> `file_ops` (list/search/read/write/edit/move/copy/delete/mkdir — admitted here
> for its read-side operations; its write side is gated by `denied_tools` and the
> sandbox path policy).

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
