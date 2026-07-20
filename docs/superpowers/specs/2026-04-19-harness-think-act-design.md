# Harness (Think→Act Loop) Design — Managed-Agents Phase 4

**Date:** 2026-04-19
**Phase:** 4 of the managed-agents refactor roadmap
**Parent roadmap:** [2026-04-18-managed-agents-refactor-roadmap.md](./2026-04-18-managed-agents-refactor-roadmap.md) §8 Phase 4
**Predecessors:** Phase 0 (AcpAdapter, `290cb6b4f`) · Phase 1 (SessionService, `0791afcaf`) · Phase 2 (ToolService, `47db24bf2`) · Phase 3 (Sandbox, `2ee280ab2`)

---

## 1. Goal

Rewrite `src/agent_loop/` (currently 40,082 LOC / 30 files, dominated by the 4,559-line `loop_core.rs`) into a minimal managed-agents-style Harness: a stateless Think→Act driver that composes over the decoupled SessionService (Phase 1) + ToolService (Phase 2) + Sandbox (Phase 3), matching Anthropic's blog primitives.

The new `src/harness/` module must be < 600 lines total (see §5 for per-file budgets), with cross-cutting concerns relocated to their proper homes rather than inlined into the loop.

## 2. Non-Goals

- Orchestrator / Flow layer (Phase 5)
- Legacy `src/agent_loop/` deletion (Phase 6)
- `stop_hooks` / `verify_stop_hook` / `integration_probe` relocation (Phase 5)
- `subagent_tool` / `subagent_teammates` relocation (Phase 5)
- Concurrent tool calls within a turn (extension point reserved; implementation deferred)

## 3. Principles Alignment

- **R8 LLM Sovereignty** — Harness contains zero rule-engine middleware; all judgment lives in the model.
- **R10 Intelligence Lives in the Prompt** — Any "intelligence" displaced from the old loop goes into system prompt templates, not new middleware layers.
- **R3 Core Minimalism** — Target < 400 lines for the loop core.
- **P4 Dependency Inversion** — Harness depends on `SessionService` / `ToolService` / `LlmProvider` / `SandboxFactory` traits, never concrete types.
- **P6 Simplicity (KISS + YAGNI)** — No extension points without current usage except the documented `Vec<ToolCall>` seam for future concurrency.

## 4. Architecture

### 4.1 Phase 4a — Relocation (Strangler, Old Loop Still In Place)

The old `agent_loop/loop_core.rs` keeps driving production traffic. Inlined cross-cutting concerns are extracted to their proper homes one PR at a time. Each PR is independently shippable and must keep `cargo test` green plus not regress the pre-existing test baseline inherited from Phase 3 (9059 pass / 2 known-fail / 20 ignored).

| Current location (`agent_loop/`) | Target destination | Rationale |
|---|---|---|
| `context_budget/`, `compaction/`, `truncation_recovery.rs`, `sections/` | `ToolService` pre-LLM middleware | These transform the LLM input before generation; belong to ToolService. |
| `retry.rs`, `safety.rs` (ingress hard filter) | LLM provider layer / Session ingress hook | Retry is provider / tool concern; safety filtering is a Session boundary. |
| `streaming_bridge.rs` | Session layer event emit helpers | Streaming is event delivery, not loop logic. |
| H1 — ApprovalRequester adapter | `src/approval/` (module) | Phase 3 carry-over: shape-translate legacy `ApprovalRequest` ↔ tool-level `ApprovalRequester`. |
| H2 — SESSION_ID scope propagation | cron / heartbeat / direct-dispatch entry points | All tool entries must set the task_local scope. |
| H4 — exec-class exclude-list | `LayeredPermissionResolver` | Phase 3 carry-over: exec-class tools bypass `PermissionLayer` approval; only `WorkspaceSandbox` asks. |
| `tool_pipeline.rs`, `tool_orchestrator.rs`, `tool_execution_context.rs` | Partially merged into Phase 2 `ToolService`; residue evaluated and either merged or documented for Phase 5/6 | Closes unfinished Phase 2 work. |
| `stop_hooks.rs`, `verify_stop_hook.rs`, `integration_probe.rs` | **Stay in `agent_loop/` this phase** — moved in Phase 5 | Phase 4 does not touch Orchestrator boundary. |
| `subagent_tool.rs`, `subagent_teammates*.rs` | **Stay in `agent_loop/` this phase** — moved in Phase 5 | Same. |

**Phase 4a exit criterion:** `loop_core.rs` shrinks from 4,559 lines to < 1,500 lines (the remainder is the `while !done { ... }` shell plus un-migrated stop_hooks / subagent plumbing).

### 4.2 Phase 4b — New Harness + Cut-Over

- New `src/harness/` module with `Harness` trait + `AgentHarness` implementation.
- `AppContext` assembly chooses between old `agent_loop` path and new `AgentHarness` based on the `ALEPH_HARNESS_V2` environment variable (default: off).
- Ship with default off; author manually exercises v2 for a release cycle; next version flips default on; Phase 6 deletes the old `agent_loop/`.

## 5. Module Layout

```
src/harness/
├── mod.rs              // pub exports
├── trait_def.rs        // Harness trait + TurnState + HarnessError
├── agent.rs            // AgentHarness implementation
├── deps.rs             // HarnessDeps (dependency bundle)
└── tests/              // unit + integration tests
```

File budgets:
- `agent.rs` < 250 lines
- Each other file < 150 lines
- Whole module < 600 lines

## 6. Harness Trait

```rust
#[async_trait]
pub trait Harness: Send + Sync {
    /// Run one Think→Act turn; returns whether the session should continue.
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError>;

    /// Convenience: loop run_turn until Done.
    async fn run(&self, session_id: &SessionId) -> Result<(), HarnessError> {
        loop {
            match self.run_turn(session_id).await? {
                TurnState::Continue => continue,
                TurnState::Done => return Ok(()),
            }
        }
    }
}

pub enum TurnState { Continue, Done }

pub enum HarnessError {
    Llm(LlmError),
    Tool(ToolError),
    Session(SessionError),
    Cancelled,
}
```

**Design choices:**
- **Constructor injection** (`AgentHarness::new(deps)`) — long-lived component, dependencies assembled at startup.
- **Both `run_turn` and `run`** — `run_turn` is the atomic unit enabling Phase 5 Orchestrator step-through / interleave; `run` is the default loop convenience.
- **`Result<TurnState, HarnessError>`** — `Done` is normal termination, not an error; hard failures surface via `HarnessError`.

## 7. AgentHarness Implementation Sketch

```rust
pub struct AgentHarness {
    session: Arc<dyn SessionService>,
    tools: Arc<dyn ToolService>,
    sandbox_factory: Arc<dyn SandboxFactory>,
    llm: Arc<dyn LlmProvider>,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self { /* unpack */ }
}

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError> {
        // 1. Think: read event tail, build prompt, call LLM.
        let events = self.session.tail_since_last_assistant(session_id).await?;
        let prompt = build_prompt(events);
        let response = self.llm.complete(prompt).await?;

        // 2. Emit AssistantMessage.
        self.session
            .emit(session_id, SessionEvent::AssistantMessage(response.clone()))
            .await?;

        // 3. No tool_use → Done.
        let tool_calls: Vec<ToolCall> = response.tool_uses();
        if tool_calls.is_empty() {
            return Ok(TurnState::Done);
        }

        // 4. Act: sequential execution, concurrency seam reserved.
        self.act(session_id, tool_calls).await?;
        Ok(TurnState::Continue)
    }
}

impl AgentHarness {
    async fn act(&self, session_id: &SessionId, tool_calls: Vec<ToolCall>)
        -> Result<(), HarnessError>
    {
        for call in tool_calls {
            let result = self.tools.invoke(session_id, call).await?;
            self.session.emit(session_id, SessionEvent::ToolResult(result)).await?;
        }
        Ok(())
    }
}
```

**Invariants:**
- Harness holds zero per-session state (stateless).
- Harness never touches `Sandbox` directly; Sandbox is owned by `ToolService` (injected via `sandbox_factory` at ToolService construction).
- All failures flow through `HarnessError`; upstream decides whether to emit `SessionEvent::Error` or retry.
- `act()` receives `Vec<ToolCall>` to preserve the future concurrency seam; today the body is a for-loop.

## 8. Concurrent Tool Calls — Deferred

The blog notes that tool calls within a turn can be parallel. This spec keeps `act()` strictly sequential today. Rationale:
- Real-world latency bottleneck is not tool dispatch.
- Parallel completion ordering, approval UX under parallel prompts, and ToolService/Sandbox concurrency safety are not designed.
- Interface (`Vec<ToolCall>`) is future-proof; when concurrency is needed (Phase 5+), only the `act()` body changes.

## 9. SessionEvent Schema — No Changes

Harness reuses existing variants (`UserMessage`, `AssistantMessage`, `ToolUse`, `ToolResult`, `Error`, `SystemMessage`). Turn boundaries are expressed implicitly by the event sequence; `run_turn`'s return value is the turn boundary for the driver.

No new `TurnStarted` / `TurnEnded` / `HarnessStep` variants. If Phase 5 Orchestrator later needs explicit boundary events, they are added then (YAGNI).

## 10. Phase 3 Carry-Over Work — Detail

### H1 — ApprovalRequester Adapter

- New `ChannelApprovalBridgeAdapter` implements the tool-level `ApprovalRequester` trait.
- Maps tool-level `{ tool_name, reason }` to legacy `ApprovalRequest { command, cwd, session_key, ... }` using the current SESSION_ID-scoped workspace for `cwd` and the live `session_key`.
- Replaces Phase 3's fallback (`tracing::warn! + Denied`) at all exec-class tool approval sites.
- **Validation:** exec-class tool under `Ask` policy produces exactly one correctly-shaped approval prompt.

### H2 — SESSION_ID Scope Propagation

- Audit all tool entry points: `invoke_with_session_trace` (existing scope setter), cron scheduler, heartbeat dispatcher, direct-dispatch paths.
- Extract a unified helper `with_session_scope(session_id, fut)` that sets the task_local.
- Replace every direct tool invocation with a call through the helper.
- **Validation:** cron-path integration test invoking `CodeExecTool` no longer reports "no active session context".

### H4 — Exec-Class Exclude-List

- Add an exec-class tool registry to `LayeredPermissionResolver` (e.g., `CodeExecTool`).
- These tools in `PermissionLayer` perform policy resolution only — **no** call to `ApprovalGate::request_approval_for_tool`.
- Approval is solely owned downstream by `WorkspaceSandbox`.
- **Validation:** exec tool under `Ask` policy produces exactly one approval prompt, not two.

## 11. PR Slicing — Phase 4a

Each PR is independent; `cargo test` green and baseline not regressed after each.

1. **4a.1** — H2 SESSION_ID scope propagation (first; others may depend).
2. **4a.2** — H1 ApprovalRequester adapter.
3. **4a.3** — H4 exec-class exclude-list.
4. **4a.4** — Context pre-processing → `ToolService` pre-LLM middleware (`context_budget` + `compaction` + `truncation_recovery` + `sections`). Introduce minimal `PreLLMMiddleware` trait with one method `async fn transform(messages) -> messages`; do not over-abstract.
5. **4a.5** — `retry` / `safety` / `streaming_bridge` relocation to their proper homes.
6. **4a.6** — Residue from `tool_pipeline.rs` / `tool_orchestrator.rs` / `tool_execution_context.rs`; merge what belongs in Phase 2 `ToolService`, document the rest for Phase 5/6.

## 12. PR Slicing — Phase 4b

Starts after all 4a PRs merged.

1. **4b.1** — Trait skeleton: `src/harness/{mod.rs, trait_def.rs, deps.rs}`; `AgentHarness::new()` stub; `run_turn` = `todo!()`; TDD entry = doc-test on the trait shape.
2. **4b.2** — AgentHarness Think phase: read event tail → build_prompt → LLM → emit AssistantMessage; return `Done` if no tool_use. TDD: mock `SessionService` + `LlmProvider`.
3. **4b.3** — AgentHarness Act phase: sequential for-loop over tool_uses; emit `ToolResult`; `HarnessError` classification on failure. TDD: mock `ToolService`.
4. **4b.4** — `run` loop integration test: real `SessionService` + mocked LLM + mocked ToolService; end-to-end multi-turn session.
5. **4b.5** — `AppContext` assembly reads `ALEPH_HARNESS_V2`; wire both paths behind the flag (default off). Run full integration test suite under both `ALEPH_HARNESS_V2=0` and `ALEPH_HARNESS_V2=1`; baseline must not regress.
6. **4b.6** — Manual E2E: run `target/release/aleph-server start` with `ALEPH_HARNESS_V2=1`; exercise chat / tools / cron / heartbeat. CHANGELOG entry: "v2 harness available via `ALEPH_HARNESS_V2` (opt-in)". Next version flips default on; Phase 6 deletes old `agent_loop/`.

## 13. Testing Strategy

### 13.1 Unit Tests (`src/harness/tests/`)

- **Think phase:** mock `SessionService` (fixed event tail) + mock `LlmProvider` (fixed AssistantMessage) → assert emitted event sequence, `TurnState` return.
- **Act phase:** mock `ToolService` (inject success/failure) → assert tool invocation order, `ToolResult` emit order, `HarnessError` classification.
- **Error paths:** `LlmProvider` error → `HarnessError::Llm`; `ToolService` error → `HarnessError::Tool`; `SessionService` emit error → `HarnessError::Session`.
- **Loop termination:** LLM response without tool_use → `Done`; with tool_use → `Continue`.
- **Concurrency seam:** `act()` given empty / single / multi-element `Vec<ToolCall>`.

### 13.2 Integration Test

- Real `SessionService` + scripted `LlmProvider` + mocked `ToolService` → run a complete session: `UserMessage → [Think→Act]×N → Done`.
- Assert final event log structure (AssistantMessage / ToolResult / Error order).
- Multi-session isolation: run several sessions, assert independent event logs.

### 13.3 Regression Protection

- CI matrix: one lane `ALEPH_HARNESS_V2=0`, one lane `ALEPH_HARNESS_V2=1`; both must be green.
- Baseline (9059 pass / 2 known-fail / 20 ignored) must not regress under either setting.

### 13.4 Phase 4a Relocation Tests

- Each relocation PR keeps existing tests green (structural change, not behavioral).
- H1 / H2 / H4 add targeted tests:
  - **H1:** exec tool under `Ask` policy produces one correctly-shaped approval.
  - **H2:** cron-path `CodeExecTool` does not fail with "no active session context".
  - **H4:** exec tool under `Ask` policy produces exactly one approval, not two.

### 13.5 Out of Scope

- LLM actual output quality (provider layer's concern).
- `SessionService` persistence internals (Phase 1's concern).
- `Sandbox` / `ToolService` internals (Phase 2/3's concern).

## 14. Risks

- **R1 — Behavioral drift during relocation.** 4a moves budget/compaction/truncation into middleware; execution path changes. *Mitigation:* one concern per PR, `cargo test` green after each, baseline not regressed.
- **R2 — `PreLLMMiddleware` over-abstraction.** Generic parameters / lifetimes / trait composition would violate P6. *Mitigation:* trait holds exactly one method `async fn transform(messages) -> messages`; extend only when real pressure appears.
- **R3 — Cut-over behavioral divergence.** v2 may disagree with v1 on edge cases (e.g., empty tool_use → Done). *Mitigation:* 4b.5 matrix tests both flags; manual E2E before ship; ship default-off for one release.
- **R4 — 4b still depends on un-relocated stop_hooks / subagent.** These are Phase 5. *Mitigation:* accepted as tech debt; cleaned up alongside Phase 5 Orchestrator work.

## 15. Open Questions (Spec-Phase Pending)

- **Q15.1 — `build_prompt(events)` signature.** Events are presumed already post-middleware (budget/compaction/persona applied). Confirm during 4b.2 whether Harness needs any agent-specific shaping or truly just `events → messages`.
- **Q15.2 — Cancellation semantics.** No `CancellationToken` in `run_turn` today; rely on tokio task abort. If in-flight cancellation mid-turn becomes necessary, revisit.
- **Q15.3 — `tail_since_last_assistant` location.** Prefer this method on `SessionService` (keeps Harness stateless). Confirm interface during 4b.2; if the method does not exist, add it to SessionService rather than tracking cursor state in Harness.

## 16. Success Criteria

- `src/harness/` total < 600 lines; `agent.rs` < 250 lines.
- Old `loop_core.rs` shrinks from 4,559 to < 1,500 lines by end of 4a.
- `cargo test` green under both `ALEPH_HARNESS_V2=0` and `ALEPH_HARNESS_V2=1`; baseline not regressed.
- H1 / H2 / H4 resolved with targeted tests passing.
- Manual E2E under `ALEPH_HARNESS_V2=1` exercises chat / tools / cron / heartbeat without regression.
- Phase 5 (Orchestrator) can build on top of `run_turn` without modifying the Harness trait.
