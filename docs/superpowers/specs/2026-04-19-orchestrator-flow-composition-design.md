# Phase 5 — Orchestrator & Flow Composition Design

**Date:** 2026-04-19
**Parent roadmap:** `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md` §9
**Prior phases (landed on `main`):**
- Phase 0 (AcpAdapter) — `290cb6b4f`
- Phase 1 (SessionService) — `0791afcaf`
- Phase 2 (ToolService) — `47db24bf2`
- Phase 3 (Sandbox + LayeredPermissionResolver) — `2ee280ab2`
- Phase 4 (Harness Think→Act + SessionDriver) — landed; `ALEPH_HARNESS_V2=1` opt-in discoverability

**Out of scope:** legacy deletion (Phase 6); Process/Container sandboxes; semantic unification of teams/sub_agents/groups (Roadmap §12).

---

## 1. Goal

Introduce `src/orchestrator/` as the single dispatch point between Gateway and `AgentHarness`. Add a declarative `FlowSpec` that composes over the existing `AgentDef` and wires brain (LLM provider), sandbox kind, and session strategy. All multi-agent call paths (chat, sub-agent, team, swarm) share one codepath: **Gateway → Orchestrator → Harness**.

## 2. Background & Constraints

- Roadmap §9 defines the scope: FlowSpec, preset + user-defined flows, Orchestrator owning session lifecycle + Harness dispatch + Sandbox provisioning.
- §10 risk mitigation mandates a *narrow* Orchestrator (session lifecycle + dispatch only).
- §12 "Not now" constraint: Phase 5 unifies the **plumbing only**; semantic unification of `teams`/`sub_agents`/`groups` is a follow-up effort.
- R9 (LLM sovereignty): flow selection and composition stay tool-driven — the LLM picks flows, not a rule engine.
- R10 (intelligence in prompts): orchestrator does not re-implement planning or intent classification.

## 3. Key Decisions (from brainstorming)

| # | Decision | Resolution |
|---|---|---|
| 3.1 | Relation between `FlowSpec` and existing `AgentDef` | **Compose** — `FlowSpec` references `AgentDef` by id and layers `brain` / `sandbox_kind` / `session_strategy` on top. `AgentDef.mode` and `AgentDef.max_iterations` stay on `AgentDef` as defaults; `FlowSpec.overrides` can override. |
| 3.2 | `SessionStrategy` variants | Three variants: `Reuse` (interactive chat, default), `Fresh` (one-shot/cron), `Child { parent_session_key }` (sub-agent dispatch). |
| 3.3 | `SandboxKind` variants | `None` (exec tools denied) and `Workspace` (route exec tools to `WorkspaceSandbox`). Future `Process`/`Container` get their own variants when shipped. |
| 3.4 | `BrainRef` shape | Three-variant enum: `Default` (global fallback chain), `Preferred { provider }` (try preferred, fallback on failure), `Strict { provider, model }` (no fallback). Existing agents migrate to `Default` for zero behavior change. |
| 3.5 | teams / swarm / sub_agents migration | **Adapter shim (Option B)** — keep teams' SQLite persistence + swarm's DAG/bus intact; only swap ~8 "spawn child agent" call sites to `orchestrator.dispatch`. |
| 3.6 | `flow_run` as LLM tool | **Yes, opt-in** — flows are invocable by LLM via `flow_run` tool. Guardrails: (1) `AgentDef.allowed_tools` must explicitly list `flow_run`; (2) `MAX_FLOW_DEPTH = 4`; (3) child flow uses its own FlowSpec's permission/sandbox, does not inherit parent. |
| 3.7 | Config format | **TOML** (consistent with `aleph.toml`). Resolves Roadmap §11 Q11.6. |
| 3.8 | Hot-reload of flows | **Explicit reload RPC only (Option B)** — `gateway.flow.reload` / `aleph reload flows` triggers re-read. `FlowRegistry` uses `ArcSwap<FlowSet>` for lock-free atomic replace. No file watcher in v1. |

## 4. Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ Gateway (OpenAI API / Telegram / Slack / CLI / ...)           │
│ Translates request → FlowRequest{ flow_id, input, channel }  │
└────────────────────────────┬─────────────────────────────────┘
                             ▼
┌──────────────────────────────────────────────────────────────┐
│ Orchestrator (src/orchestrator/)           — Phase 5 new     │
│ ─────────────────────────────────────────────────────────── │
│ 1. FlowRegistry.resolve(flow_id) → FlowSpec                   │
│ 2. SessionStrategy → allocate/reuse/fork SessionKey           │
│ 3. BrainRef → pick LLM (fallback-aware or strict)             │
│ 4. SandboxKind → provision Sandbox lazily                     │
│ 5. Instantiate AgentHarness with injected deps                │
│ 6. Pipe SessionEvent stream → Gateway sink                    │
└──────────────┬────────────────────────────────┬──────────────┘
               ▼                                ▼
┌──────────────────────────────┐  ┌──────────────────────────┐
│ AgentHarness (Phase 4)        │  │ SessionService (Phase 1)  │
│ Think → Act loop              │  │ Event log persistence     │
└────────┬──────────────────────┘  └──────────────────────────┘
         ▼
┌────────────────────────┐  ┌──────────────────────┐
│ ToolService (Phase 2)   │  │ Sandbox (Phase 3)     │
│ builtin + MCP + plugin  │  │ Workspace / None       │
└────────────────────────┘  └──────────────────────┘
```

**Orchestrator has exactly six responsibilities:** resolve / session / brain / sandbox / dispatch Harness / stream pipe.
**It does NOT own:** think/act loop (Phase 4); tool execution (Phase 2); event storage (Phase 1); sandbox implementation (Phase 3).

## 5. `FlowSpec` Schema

`src/orchestrator/flow_spec.rs`:

```rust
pub type FlowId = String;
pub type AgentId = String;
pub type ProviderId = String;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    pub id: FlowId,
    pub description: String,
    pub agent: AgentId,
    pub brain: BrainRef,
    pub sandbox_kind: SandboxKind,
    pub session_strategy: SessionStrategy,
    #[serde(default)]
    pub overrides: FlowOverrides,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BrainRef {
    Default,
    Preferred { provider: ProviderId },
    Strict    { provider: ProviderId, model: Option<String> },
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxKind {
    None,
    Workspace,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SessionStrategy {
    Reuse,
    Fresh,
    Child { parent_session_key: Option<String> },
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowOverrides {
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub context_mode:   Option<crate::agents::types::ContextMode>,
    #[serde(default)]
    pub extra_system_prompt: Option<String>,
}
```

**TOML example (preset `default.toml`):**

```toml
[[flow]]
id = "default-agent"
description = "Primary chat agent for user-facing interactions"
agent = "main"
sandbox_kind = "workspace"

[flow.brain]
kind = "default"

[flow.session_strategy]
kind = "reuse"

[[flow]]
id = "researcher"
description = "Read-only web & document researcher"
agent = "researcher"
sandbox_kind = "none"

[flow.brain]
kind = "preferred"
provider = "minimax"

[flow.session_strategy]
kind = "child"

[flow.overrides]
max_iterations = 10
```

**Schema invariants:**
- `agent` is an id reference, not inlined — updating `AgentDef.allowed_tools` propagates to every `FlowSpec` that references it.
- `SessionStrategy::Child.parent_session_key` may be `None` in TOML; Orchestrator injects the parent at dispatch time from the calling context.
- No field encodes "flow composition" (e.g., chain A→B→C). Composition happens via the `flow_run` LLM tool at runtime, enforcing R9.
- All enums are `#[non_exhaustive]` + `deny_unknown_fields` to catch schema drift early.

## 6. Orchestrator API & Dispatch

`src/orchestrator/mod.rs` + `dispatch.rs`:

```rust
pub struct Orchestrator {
    flow_registry:     Arc<FlowRegistry>,
    agent_registry:    Arc<AgentRegistry>,
    session_service:   Arc<dyn SessionService>,
    tool_service:      Arc<dyn ToolService>,
    sandbox_factory:   SandboxFactory,   // see note below
    provider_registry: Arc<ProviderRegistry>,
}

/// Phase 5 introduces this thin allocator because the existing
/// `build_sandbox()` factory returns a single shared `Arc<dyn Sandbox>`,
/// while Orchestrator needs per-session provisioning (Roadmap §9:
/// "Sandbox provisioning on demand"). Implemented as a type alias
/// over a boxed closure, not a new trait — keeps the sandbox crate
/// boundary unchanged.
pub type SandboxFactory =
    Arc<dyn Fn(SandboxKind, &SessionKey) -> Result<Arc<dyn Sandbox>, FlowError> + Send + Sync>;

pub struct FlowRequest {
    pub flow_id:        FlowId,
    pub input:          FlowInput,
    pub channel:        Option<PeerChannel>,
    pub session_hint:   Option<SessionKey>,
    pub parent_session: Option<SessionKey>,
    pub depth:          u8,
}

/// Gateway-agnostic input envelope. Carries either a single user prompt
/// (interactive chat) or a pre-assembled message vector (OpenAI-compatible
/// `/v1/chat/completions` request, cron payload, tool result replay).
#[derive(Debug, Clone)]
pub enum FlowInput {
    Prompt(String),
    Messages(Vec<crate::session::events::MessageContent>),
}

pub struct FlowHandle {
    pub session_id: SessionId,
    pub events:     broadcast::Receiver<SessionEvent>,
    pub completion: oneshot::Receiver<FlowOutcome>,
    pub cancel:     CancellationToken,
}

impl Orchestrator {
    pub async fn dispatch(&self, req: FlowRequest) -> Result<FlowHandle, FlowError>;
    pub async fn reload_flows(&self) -> Result<(), FlowError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("unknown flow id: {0}")]   UnknownFlow(FlowId),
    #[error("unknown agent id: {0}")]  UnknownAgent(AgentId),
    #[error("flow recursion limit ({max}) exceeded")]
    RecursionLimit { max: u8 },
    #[error("session {0} already dispatching")]
    SessionConflict(SessionKey),
    #[error("sandbox provision failed: {0}")]
    SandboxProvisionFailed(String),
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(ProviderId),
    #[error("invalid flow config: {0}")]
    InvalidConfig(String),
}
```

**Seven-step dispatch (each step independently unit-testable):**

1. `flow_registry.resolve(req.flow_id)` → `FlowSpec` (unknown → `UnknownFlow`)
2. `depth_guard(req.depth, MAX_FLOW_DEPTH=4)` (over → `RecursionLimit`)
3. `agent_registry.get(flow.agent)` (unknown → `UnknownAgent`)
4. `session_key = resolve_session(flow.session_strategy, req)` — acquire per-session lock
5. `llm = provider_registry.pick(flow.brain)` — three-variant dispatch
6. `sandbox = match flow.sandbox_kind { None => NoopSandbox, Workspace => factory.provision(session_key) }`
7. `spawn harness::run(session_key, deps{agent,llm,tool_service,sandbox}, sink) → FlowHandle`

**Concurrency invariants:**
- Orchestrator is stateless; all session state lives behind `SessionService`.
- Same `SessionKey` must not be concurrently dispatched — `resolve_session` acquires a per-session lock. `Child` strategy always allocates a fresh `SessionKey`, so teams dispatching N sub-agents in parallel never collides on the parent lock.
- Cancellation: Gateway drops the `CancellationToken` → Harness exits at the next Think/Act boundary → `SessionEvent::Cancelled` is appended.

## 7. `flow_run` LLM Tool

`src/orchestrator/flow_run_tool.rs` registers a tool with ToolService:

```
Name:        flow_run
Description: Invoke a sub-flow and return its final text output.
Schema:      { flow_id: string, input: string }
```

Execution:
1. Read `parent_session_key` + `current_depth` from the current session context.
2. Construct `FlowRequest { depth: current_depth + 1, parent_session: Some(parent_key), session_strategy inherited from the called flow's FlowSpec }`.
3. `orchestrator.dispatch(req).await` → `FlowHandle`.
4. `handle.completion.await` → return `outcome.final_text` as the tool result.

Guardrails:
- **Opt-in.** `AgentDef.allowed_tools` must explicitly include `"flow_run"`. Default `default-agent` preset includes it; `researcher`/`explore` do not.
- **MAX_FLOW_DEPTH = 4** — hardcoded in `src/orchestrator/resolver.rs`, not configurable.
- **No inheritance.** Child flow's `sandbox_kind` / `tools_allowed` / `brain` come from its own `FlowSpec`, not the parent. This means a parent's Strict brain does not force the child to use the same provider.

## 8. Gateway Wiring

Current `src/gateway/execution_engine/run_loop.rs::run_agent_loop` (~1500 lines) builds `AgentLoop` directly. Phase 5 replaces the core:

```rust
// After Phase 5
pub async fn run_agent_loop(req, agent, tools, provider, sink) -> Result<RunStatus> {
    let flow_id = flow_resolver::resolve(&req.agent_id, req.peer_channel.as_ref());
    let handle = orchestrator.dispatch(FlowRequest {
        flow_id,
        input: req.input,
        channel: req.peer_channel,
        session_hint: Some(req.session_key.clone()),
        parent_session: None,
        depth: 0,
    }).await?;
    pipe_events_to_sink(handle.events, sink).await;
    let outcome = handle.completion.await?;
    Ok(outcome.into())
}
```

**`flow_resolver` default rules** (in `src/orchestrator/resolver.rs`, overridable via `aleph.toml [flow_routing]`):

```
agent_id = "main"        → "default-agent"
agent_id = "explore"     → "explore"
agent_id = "coder"       → "coder"
agent_id = "researcher"  → "researcher"
agent_id = "default"     → "default"
agent_id = "plan"        → "plan"
agent_id = "verify"      → "verify"
unknown agent_id         → FlowError::UnknownAgent
```

**Per-channel override** (user-defined in `aleph.toml`):

```toml
[flow_routing]
"main.telegram" = "main-lite"
"main.*"        = "default-agent"
```

Pattern order: exact `agent.channel` wins over wildcard `agent.*`. Unknown keys fall through to the built-in default table.

## 9. teams / swarm / sub_agents Migration

**Option B — adapter shim.** ~8 call sites total, ~150-line diff:

| File | Before | After |
|---|---|---|
| `src/agents/swarm/coordinator.rs` | `AgentLoop::new(sub_agent, ...)` | `orchestrator.dispatch(FlowRequest { session_strategy: Child, ... })` |
| `src/agents/swarm/tasks/mod.rs` | Same pattern | Same swap |
| `src/teams/sessions/coordinator.rs` | `AgentLoop::new` per team member | `orchestrator.dispatch` with `Child` strategy |
| `src/teams/plans.rs` | Plan executor spawns agents | Same swap |
| `src/gateway/execution_engine/run_loop.rs` | Builds `AgentLoop` | `orchestrator.dispatch` (see §8) |
| `src/gateway/handlers/session/mod.rs` | RPC session handler builds loop | Same swap |
| `src/agents/sub_agents/traits.rs` | `SubAgent` trait exists; default impl builds `AgentLoop` | `SubAgent` trait unchanged; default impl routes through Orchestrator |

**What stays:**
- teams' SQLite store (team membership, plans, artifacts, messages)
- swarm's bus / collective_memory / rules / task DAG
- `SubAgent` trait and its public signature

## 10. Module Structure & LOC Budget

```
src/orchestrator/
├── mod.rs                      # pub API                                        (~80 LOC)
├── flow_spec.rs                # FlowSpec + enums                               (~150 LOC)
├── flow_registry.rs            # FlowRegistry + ArcSwap reload                  (~120 LOC)
├── loader.rs                   # TOML parse: flows/*.toml + aleph.toml          (~150 LOC)
├── resolver.rs                 # (agent_id, channel) → flow_id + depth_guard   (~130 LOC)
├── dispatch.rs                 # seven-step dispatch                            (~180 LOC)
├── flow_run_tool.rs            # flow_run LLM tool                              (~100 LOC)
├── presets/
│   ├── mod.rs                  # builtin_flows()                                (~20 LOC)
│   └── default_flows.toml      # 7 preset FlowSpec entries                      (~120 TOML)
└── tests/
    ├── flow_spec_parse.rs
    ├── resolver.rs
    ├── dispatch.rs
    ├── reload.rs
    └── flow_run_tool.rs
```

**Budget:**
- Runtime Rust: **~930 LOC**; max single file: `dispatch.rs` ~180.
- Test code: ~500 LOC; coverage target ≥80% per new module.
- TOML presets: ~120 lines.

**External diff budget:**
- `src/agents/registry.rs` — 0 changes (AgentDef untouched).
- `src/gateway/execution_engine/run_loop.rs` — ~−200 / +30 lines.
- `src/agents/swarm/coordinator.rs` + `swarm/tasks/mod.rs` — ~20 lines each.
- `src/teams/sessions/coordinator.rs` + `teams/plans.rs` — ~15 lines each.
- `src/app_context.rs` + `src/bin/aleph-server/commands/start/mod.rs` — ~+40 lines total (provision Orchestrator into `AppContext`).

## 11. Testing Strategy

| Layer | Location | Covers |
|---|---|---|
| Unit | `src/orchestrator/tests/*.rs` | TOML roundtrip, resolver, depth_guard, BrainRef three-way, SandboxKind two-way, SessionStrategy three-way, every `FlowError` variant |
| Module integration | `src/orchestrator/dispatch.rs::tests` | Seven-step dispatch with mock `SessionService`, `SandboxFactory`, `ProviderRegistry` |
| Cross-module integration | `tests/orchestrator_e2e.rs` (new) | Gateway → Orchestrator → Harness → ToolService full path for `default-agent`, `researcher`, `flow_run` |
| Regression | Existing 9076 library tests + 2 `tests/harness_run_e2e.rs` | Must stay green |
| Manual E2E | `docs/superpowers/plans/*-phase-5-manual-e2e-notes.md` | Gateway OpenAI API real run: main / researcher / `flow_run` composition |

**Mandatory scenarios:**
1. `SessionStrategy::Reuse` writes to the same `SessionKey` across turns.
2. `SessionStrategy::Fresh` allocates a new `SessionKey` per dispatch.
3. `SessionStrategy::Child` sets `SessionEvent::SessionStarted.parent` correctly.
4. `flow_run` at depth 4 returns `FlowError::RecursionLimit`.
5. Child flow uses its own `sandbox_kind`; parent's kind does not leak.
6. `BrainRef::Strict` failure returns `FlowError::ProviderUnavailable` (no fallback).
7. `BrainRef::Preferred` failure falls back through the global chain.
8. `reload_flows` during an in-flight dispatch: the in-flight flow keeps the old `FlowSpec` snapshot; subsequent dispatches pick up the new one.
9. Concurrent dispatch on the same `SessionKey` — second one returns `FlowError::SessionConflict`.
10. Unknown `flow_id` returns `FlowError::UnknownFlow`.
11. `[flow_routing]` override resolves `"main.telegram" → "main-lite"` for a Telegram channel request; same `main` agent on a non-telegram channel falls through to the default table.
12. `reload_flows` with a malformed TOML file returns `FlowError::InvalidConfig` and leaves the previous `FlowSet` snapshot untouched (no partial updates).

## 12. Exit Criteria (CI-verifiable)

1. `src/gateway/execution_engine/run_loop.rs::run_agent_loop` no longer instantiates `AgentLoop` directly; every path goes through `orchestrator.dispatch`.
2. `src/agents/swarm/coordinator.rs`, `src/teams/sessions/coordinator.rs`, `src/teams/plans.rs`, `src/agents/swarm/tasks/mod.rs` contain zero `AgentLoop::new` references.
3. `~/.aleph/flows/presets/default.toml` ships 7 preset FlowSpec entries mapping 1:1 to the 7 existing `AgentDef` builtins (`main`, `explore`, `coder`, `researcher`, `default`, `plan`, `verify`).
4. `flow_run` tool is registered on `ToolService`; `default-agent` preset lists `"flow_run"` in its AgentDef's `allowed_tools`.
5. Baseline library tests (9076) + 2 `harness_run_e2e` tests stay green; new orchestrator unit tests (~15) + 3 new `tests/orchestrator_e2e.rs` cases green.
6. Manual E2E recorded: OpenAI API request "research X and summarize" triggers `default-agent` → `flow_run(researcher, ...)` → child flow returns text → parent summarizes.
7. `ALEPH_HARNESS_V2=1` start path still routes Orchestrator dispatch into `AgentHarness` (Phase 4 `SessionDriver`); no regression on the Phase 4 integration tests.
8. `cargo clippy -- -D warnings` clean on the worktree.
9. `grep -rn 'AgentLoop::new\|loop_core::' src/ --include='*.rs'` returns only hits inside `src/agent_loop/` itself and in call sites explicitly marked with a `// PHASE-6-LEGACY` comment (five such sites max, enumerated in the Phase 5 plan).

## 13. Risks & Mitigations

| Risk | P | I | Mitigation |
|---|---|---|---|
| Orchestrator becomes a 4000-line monolith | M | H | Hard 930-LOC runtime budget in §10; each PR review must include `wc -l src/orchestrator/` in the description |
| `flow_run` recursion loops cost budget | M | M | `MAX_FLOW_DEPTH = 4` hardcoded; scenario 4 in §11 enforces the guard |
| Per-session lock regresses team concurrency | L | M | Teams dispatch with `SessionStrategy::Child` (fresh `SessionKey` each time); parent lock is never contended by children; scenario 9 covers |
| `BrainRef::Strict` strands users when tokens expire | L | L | Strict is opt-in; all seven preset flows ship with `BrainRef::Default`; docs call out the tradeoff |
| Migration misses a Gateway call site | M | M | Exit criterion 9 is a grep-based CI gate; run it in PR checks |
| teams/swarm migration silently regresses | M | H | Migration PR split into two commits: (a) dual-path (new + old side by side, compare event streams in tests); (b) delete old path after one week clean |
| TOML schema future-breakage | L | M | Every enum `#[non_exhaustive]`; every struct `#[serde(deny_unknown_fields)]` to reject silent drift |

## 14. Non-Goals (hard scope boundary)

- ❌ Deleting `AgentLoop` / `loop_core.rs` (Phase 6).
- ❌ Unifying `teams` / `sub_agents` / `groups` semantics (Roadmap §12 follow-up).
- ❌ File-watcher-driven hot reload (only explicit reload RPC).
- ❌ New sandbox implementations (`Process`, `Container`).
- ❌ Changing `SessionService` / `ToolService` / `Sandbox` trait signatures (Phases 1–3 are frozen).
- ❌ Flipping the `ALEPH_HARNESS_V2` default (Phase 6).
- ❌ Modifying `AgentDef` fields (Option B preserves it verbatim).

## 15. Open Questions Resolved

- Roadmap §11 Q11.6 (TOML vs YAML) → **TOML** (§3.7, §5).
- Roadmap §11 Q11.7 (`flow_run` self-referential) → **yes, opt-in with three guardrails** (§3.6, §7).

## 16. Follow-ups for Phase 6

- Delete `AgentLoop` / `loop_core.rs`; delete all `// PHASE-6-LEGACY` shims.
- Delete `ALEPH_HARNESS_V2` env var; make Harness unconditional.
- Fold `SubAgent` trait into `FlowSpec` directly if the semantic unification (§12 follow-up) happens.
- Optional: add file watcher on `~/.aleph/flows/` that triggers `reload_flows()` automatically.
- Documentation rewrite: `ARCHITECTURE.md`, `AGENT_SYSTEM.md`, `MULTI_AGENT_SYSTEM.md` around the new Orchestrator surface.

## 17. Next Step

Once this spec is approved by the user, invoke `superpowers:writing-plans` to produce an implementation plan at `docs/superpowers/plans/2026-04-19-managed-agents-phase-5-orchestrator.md`.
