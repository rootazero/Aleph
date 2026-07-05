# Aleph MoA-Capability Inventory (pre-port audit vs hermes-agent MoA)

Scope: what already exists in Aleph resembling Mixture-of-Agents (parallel multi-model consultation feeding one acting model), to avoid duplicating it in the hermes MoA port. All paths absolute under `/Volumes/TBU4/Workspace/Aleph`.

**Headline finding: Aleph already ships a one-shot MoA implementation** — the `subagent` tool has `proposer_models` / `synthesize` / `aggregator_model` / `synthesis_instruction` arguments that fan the same task out across models in parallel and reduce via an aggregator sub-agent, explicitly citing the MoA paper (Wang et al. 2406.04692). It is **undocumented** in `docs/reference/` (grep for `MoA|Mixture|proposer` returns nothing). What it is NOT: hermes-style continuous per-iteration advisory injection on the live conversation.

---

## 1. `src/arena/` — SharedArena (multi-agent collaboration workspace, NOT multi-model comparison)

**Purpose**: a DDD-style shared workspace ("blackboard") where multiple *named agents* collaborate on one goal. It manages state, not LLM calls — it never invokes a model.

- `src/arena/mod.rs:1-15` — module doc: "a structured workspace where multiple agents collaborate on a shared goal".
- `src/arena/types.rs` — core types: `ArenaManifest` (goal + strategy + participants, `types.rs:214-225`); `CoordinationStrategy::Peer{coordinator}` / `Pipeline{stages}` with Kahn's-algorithm cycle check (`types.rs:116-127`, `types.rs:387-427`); `ParticipantRole` Coordinator/Worker/Observer with permission sets (`types.rs:148-195`); per-agent `ArenaSlot` holding `Artifact`s (`types.rs:495-518`); `SharedFact` knowledge base entries with confidence (`types.rs:550-593`); lifecycle Created→Active→Settling→Archived (`types.rs:97-106`).
- `src/arena/manager.rs:20-206` — `ArenaManager`: `create_arena` (distributes `ArenaHandle`s per participant), `query_arena` (JSON snapshot: goal/status/progress/slot summaries, `manager.rs:124-156`), `settle_with_facts` (drains facts for persistence to MemoryStore, archives, `manager.rs:161-206`).
- `src/arena/aggregate.rs:20-37` — `SharedArena` aggregate root (slots, progress, shared facts). Note: "aggregate" here means DDD aggregate root, **not** MoA aggregation.

**Entry points**: three builtin tools `arena_create` / `arena_query` / `arena_settle` (`src/builtin_tools/arena.rs:1-6`, tool registered via `src/executor/builtin_registry/`), gateway RPC handlers (`src/gateway/handlers/arena.rs`), server wiring (`src/bin/aleph-server/commands/start/builder/handlers/arena.rs`).

**Fan-out?** No. It never sends one prompt to multiple models; agents *choose* to write artifacts/facts into their slots. **Presentation**: JSON snapshots via `arena_query`; on settle, facts flow to long-term memory. **Feeds acting loop?** Only indirectly (an agent may call `arena_query` as a tool). Stale-comment note: `types.rs:231` references a `CollaborativeExecutor` that no longer exists in the codebase (grep finds only this comment).

## 2. `src/group_chat/` — multi-persona deliberation (closest thing to multi-model discussion, but user-facing and sequential)

**Purpose**: channel-agnostic multi-persona group discussion rendered to the user (`src/group_chat/mod.rs:1-3`).

- **Personas can run different models/providers**: `Persona { id, name, system_prompt, provider: Option<String>, model: Option<String>, thinking_level: Option<String> }` (see `src/group_chat/protocol.rs`; used at `executor.rs:264-277`). Provider resolved per-persona through `ProviderRegistry` with default fallback (`src/group_chat/executor.rs:76-91`); model/thinking stamped via `RequestPayload::with_model/with_think_level` (`executor.rs:270-277`).
- **Round flow** (`GroupChatExecutor::execute_round`, `src/group_chat/executor.rs:163-317`): (1) user msg recorded; (2) **coordinator LLM call** on the default provider returns a JSON plan of respondents+order+per-persona guidance (`build_coordinator_prompt` / `parse_coordinator_plan` / `build_fallback_plan`, `src/group_chat/coordinator.rs:22,90,106`); (3) each selected persona is invoked **sequentially** (a for-loop, not parallel — `executor.rs:244`), each seeing accumulated `prior_discussion` (`executor.rs:302`).
- Personas are **single-shot LLM calls with no tools** (raw `provider.process(RequestPayload...)`, `executor.rs:269-284`).
- **Presentation**: `GroupChatMessage` stream rendered per-channel via `GroupChatRenderer` / `GroupChatCommandParser` traits (`src/group_chat/channel.rs:18-47`, e.g. Telegram `/groupchat`); sessions persisted to `StateDatabase` (`src/group_chat/orchestrator.rs:126-139`). Lifecycle managed by `GroupChatOrchestrator` (`orchestrator.rs:29-258`, max_personas/max_rounds limits).
- **Feeds an acting agent?** No. Output goes to the human; there is no synthesis step and no injection into any agent's context.

## 3. `src/agents/` + `src/agents/swarm/` — the subagent architecture, and the EXISTING MoA

### 3a. The `subagent` tool (Spawn mode) — delegate_task equivalent + built-in one-shot MoA

- Tool struct: `src/agents/subagent_tool/mod.rs:51-116` (`SubagentTool`, a `LoopTool` named `"subagent"`). Actions: `run | check_status | cancel | list | send_message | read_inbox` (`loop_tool.rs:47`).
- **`RunArgs` already carries MoA fields** (`src/agents/subagent_tool/types.rs:42-72`): `batch_tasks`, `proposer_models` ("replicate the top-level task across these models as parallel proposers … the classic MoA shape"), `synthesize` (run ONE aggregator sub-agent over the proposals), `aggregator_model`, `synthesis_instruction`.
- **Tool description advertises it** (`loop_tool.rs:28-38`): "For Mixture-of-Agents (best-quality answers to one hard question), set 'proposer_models' … then one aggregator sub-agent folds the proposals into a single synthesized answer."
- **Execution pipeline** (`src/agents/subagent_tool/loop_tool.rs`):
  - proposer_models → BatchTask expansion: `loop_tool.rs:393-410` (explicit `batch_tasks` wins; shorthand only).
  - Sync parallel fan-out: `loop_tool.rs:512-584` — `tokio::spawn` per proposer, await all, collect `(idx, model, text)` proposals with per-task panic isolation.
  - MoA reduce: `loop_tool.rs:592-673` — aggregator agent run on `aggregator_model` (falls back to top-level `model`); returns `{status:"moa_completed", synthesis, proposer_count, results}`; on aggregator failure returns `moa_synthesis_failed` **with the raw proposals preserved** (`loop_tool.rs:664-672`).
  - Synthesis prompt: `build_synthesis_prompt` (`loop_tool.rs:853-886`) — labels each proposal with its model, instructs merge/reconcile/not-pick-one-verbatim, cites Wang et al. 2406.04692, notes R7/R9/R10 compliance in comments.
  - Guards: `synthesize` requires foreground batch (`parse.rs:247-250`); `proposer_models` requires non-empty `task` (`parse.rs:235-241`); concurrency capped by `DEFAULT_MAX_CONCURRENT_SUBAGENTS = 4` semaphore (`types.rs:5`, `mod.rs:151-153`); chain-depth recursion guard (`loop_tool.rs:414-425`).
- **Different models per subagent — yes, two mechanisms**:
  1. Per-spawn `model` string → `ModelOverrideProvider` stamps `payload.model` (`src/providers/model_override_provider.rs:27-49`); the production default provider is a `FailoverProvider` whose route walk treats `payload.model` as an *explicitly pinned request model* (`src/providers/failover/provider.rs:609`, `662-681`), so cross-vendor models resolve through the configured chain.
  2. `AgentDef.provider_hint` → `provider_overrides` map pins an entire provider per agent-type (`src/agents/runtime.rs:445-454`, wired via `SubagentTool::with_provider_overrides`, `mod.rs:174-181`).
- **Subagents are full harness runs WITH tools**: `subagent_spawner` builds a fresh `AgentHarness` with parent tools wrapped in `AllowlistToolService` gated by `AgentDef.allowed_tools` (`src/agents/subagent_spawner/mod.rs:1-12,44-103`). Builtin agent types: `main`, `explore`, `coder`, `researcher`, `default` (`src/agents/registry.rs:218+`); user/project markdown agent defs are loadable (3-tier, MULTI_AGENT_SYSTEM.md:81-136), so a **no-tool "advisor" agent def is authorable today without code changes** (empty `allowed_tools`).
- **Subagents do NOT see the parent's live conversation** — they get `task` + optional parent-authored `context_summary` in a fresh child session (`subagent_spawner/mod.rs:106-118`).
- **Background + proactive announce (R5)**: `spawn_background` (`src/agents/subagent_tool/spawn.rs:66-270`) registers with `BackgroundAgentTracker`, streams progress via `ForwardingTraceSink`, and on completion broadcasts `AlephEvent::SubAgentCompleted` on the `GlobalBus`; `src/gateway/subagent_announce.rs:44-77` delivers the result **into the parent's running session proactively** — existing plumbing for "results arrive mid-run without polling".

### 3b. `src/agents/swarm/` — not MoA

`AgentMessageBus` (in-process agent-to-agent event channel) + `CoordTaskStore` DAG backing the team subsystem (`src/agents/swarm/mod.rs:1-13`). Fan-in exists only as DAG dependency semantics ("a task that depends on several others", MULTI_AGENT_SYSTEM.md:424-429), executed by the `TeamDispatcher` (`src/teams/dispatcher/`, max_concurrent=4).

### 3c. Other delegation surfaces

`session_send` (peer delegate), 9 team tools (leader/member DAG + MessageRouter inbox + `CollaborativeSession` multi-turn dialogue with Explorer/Critic prompt-roles), `a2a_delegate` (remote A2A over JSON-RPC/SSE). See MULTI_AGENT_SYSTEM.md modes 2-4.

## 4. `src/orchestrator/` and `src/looping/` — no fan-out/aggregate

- Orchestrator dispatches **one flow → one agent → one brain**: `FlowSpec { agent, brain: BrainRef::Default|Preferred{provider}|Strict{provider, model} }` (`src/orchestrator/flow_spec.rs:46-78`). No parallel fan-out found in `dispatch.rs`. Provider/model pinning per flow exists (BrainRef), which is reusable for advisor-brain selection.
- `src/looping/mod.rs:1-6` — per-session **timer-driven repeat** (clock-gated sibling of `goal`), process-memory only. Not a fan-out pattern.

## 5. Documentation state

- `docs/reference/MULTI_AGENT_SYSTEM.md` (913 lines) documents 4 modes (Spawn/Delegate/Team/A2A) + swarm sensing + dispatcher + subagent progress streaming. **Zero mention of MoA / proposer_models / synthesize** — the existing MoA feature is undocumented. Also stale: the tool table lists `subagent_spawn/steer/kill` (`MULTI_AGENT_SYSTEM.md:458-462`) but the actual tool is a single `subagent` tool with actions run/check_status/cancel/list (no steer action; grep confirms `subagent_steer` appears nowhere in `src/`).
- `docs/reference/AGENT_SYSTEM.md` (818 lines) — single-agent harness internals: AgentHarness state machine, busy-input modes (Steer/Interrupt/Queue), Thinker provider fallback, streaming, dispatcher task graph.

## 6. VERDICT — overlap with hermes MoA (advisors = no-tool advisory calls on CURRENT conversation state, injected as private guidance into the acting model's context each iteration)

| Subsystem | Overlap | Assessment |
|---|---|---|
| `src/arena/` | **LOW (~5%)** | Shared-state blackboard for named agents; no model fan-out, no LLM calls, no context injection. Irrelevant to MoA port; do not build on it. |
| `src/group_chat/` | **LOW-MEDIUM (~25%)** | Has per-persona provider/model/thinking overrides and cheap no-tool single-shot persona calls — the exact *call shape* hermes advisors need (`executor.rs:264-284` is the reference pattern). But: sequential not parallel, output is user-facing chat (not private guidance), personas never see an acting agent's conversation, no aggregation, no loop feedback. |
| `subagent` tool MoA (`proposer_models`+`synthesize`) | **HIGH (~60%) for one-shot MoA; LOW for hermes-style continuous MoA** | Already implements parallel multi-model fan-out + LLM aggregator reduce, model-invoked (R7/R8-conformant). The reduce prompt, model-pinning plumbing, concurrency caps, panic isolation, and proposal-preserving failure path all exist and should NOT be duplicated. |

**What is genuinely missing in Aleph relative to hermes MoA:**

1. **Per-iteration cadence** — Aleph MoA fires once, when the acting model chooses to call the tool. There is no harness-driven "consult advisors before/after every Think turn". (R10 explicitly forbids putting this *in* `src/harness/`; CLAUDE.md's A2 clause notes hermes fat-harness patterns are deliberately not ported wholesale. A port must live outside the 12-file harness budget — e.g., gateway/execution-engine side or a prompt-assembly layer.)
2. **Advisors seeing the CURRENT conversation** — proposers get a fresh `task` + optional parent-authored `context_summary`; nothing automatically exports the live transcript snapshot to a consultation call. The prompt builder re-reads the full event log every turn (`src/harness/agent/prompt.rs:43`), so the transcript is available — the export seam is what's missing.
3. **Private guidance injection channel** — the existing injection seams are: (a) mid-loop steering, which appends a `UserMessage` wrapped in `<system-reminder>` to a *running* session, consumed at the next turn boundary (`src/gateway/execution_engine/steering.rs:1-30` — "the Think loop re-reads the full event log every turn"); (b) `subagent_announce` delivering background results into the parent session (`src/gateway/subagent_announce.rs`). Both write persisted session events visible in the transcript; hermes-style *ephemeral, per-turn, non-persisted* private advice would be a new prompt-section concept.
4. **A dedicated no-tool advisory call path in the subagent pipeline** — every proposer today is a full `AgentHarness` run carrying tool schemas (token cost + latency). A raw `provider.process()` advisory call (as group_chat personas do) is cheaper; alternatively a zero-tool `advisor` AgentDef is authorable today with no code change, but still pays harness overhead.
5. **Standing advisor panel** — hermes advisors persist across iterations; Aleph proposers are ephemeral one-shots with no memory between calls.

**Recommendation for the port**: reuse (a) `proposer_models`/`build_synthesis_prompt` reduce machinery and `ModelOverrideProvider`/failover model-pinning for advisor model selection, (b) the group_chat persona-call shape for cheap no-tool advisory inference, (c) the steering/announce seams as the delivery mechanism into a live run. The genuinely new work is the consultation *trigger* (cadence policy), transcript-snapshot export to advisors, and the ephemeral-guidance prompt section — all of which must be placed outside `src/harness/` per R10.