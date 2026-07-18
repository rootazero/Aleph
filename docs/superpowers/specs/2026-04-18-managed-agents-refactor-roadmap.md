# Managed Agents Refactor — Roadmap

**Date**: 2026-04-18
**Status**: Roadmap (parent spec for 6 sub-specs)
**Scope**: Aleph Core
**Owner**: @rootazero
**Inspired by**: [Anthropic — How we built our multi-agent research system](https://www.anthropic.com/engineering/managed-agents)

---

## 1. Goals

Align Aleph's agent execution architecture with Anthropic's managed-agents paradigm, decomposing the current monolithic `agent_loop/loop_core.rs` (4447 lines) + in-process `SessionManager` into five decoupled, independently composable components:

- **Session** — append-only event log, durable, accessible as a service
- **Harness** — stateless Think→Act loop; crash-recoverable via `wake(session_id)`
- **Sandbox** — execution environment for tools that need one; provisioned on-demand
- **Tools** — unified `execute(name, input) → output` interface across builtin / MCP / extension
- **Orchestrator** — owns session lifecycle, harness dispatch, sandbox provisioning, flow composition

Each component replaceable behind a trait boundary; flows dynamically composable; all observable via the existing event bus.

## 2. Non-Goals

- **Not multi-tenant / multi-machine**. Aleph remains single-user self-hosted; no brain/hand pool across hosts.
- **Not container-based sandbox**. No Docker/Podman dependency; workspace-level isolation only.
- **Not a cross-process daemon**. Session service stays in-process (actor behind trait), with trait shape that permits cross-process later.
- **Not a rewrite of LLM provider layer**. Multi-provider routing, model discovery, vault remain untouched.
- **Not changing Gateway protocol**. JSON-RPC surface stays; internals swap under it.

## 3. Anthropic-Aligned Terminology

| Term | Anthropic meaning | Aleph current usage | After this refactor |
|------|-------------------|---------------------|---------------------|
| **Harness** | The loop calling Claude + routing tool calls | `AcpHarness` (external CLI wrapper) | Think→Act loop in `src/harness/`; the old `AcpHarness` is renamed `AcpAdapter` |
| **Sandbox** | Execution environment where the agent runs code / edits files | `SandboxManager` (OS seatbelt driver) + `ExecSecurityGate` + `ApprovalGate` — scattered | New `Sandbox` trait in `src/sandbox/` sits **above** the current OS-sandbox primitives; rename of `SandboxManager` deferred to Phase 3 (decided in context with the trait) |
| **Session** | Append-only event log, external to harness | `SessionManager` (in-process SQLite store) | `SessionService` trait in `src/session/`, backed by in-process actor |
| **Tools** | Unified `execute()` surface for all capabilities | Split across builtin / MCP / extension registries | Single `ToolService` façade over existing registries |
| **Orchestrator** | Infra managing session, sandbox, routing | Implicit, scattered in `agent_loop/loop_core.rs` | Explicit `src/orchestrator/` module |

Canonical glossary: `docs/reference/GLOSSARY.md` (created in Phase 0).

## 4. Aleph-Specific Constraints

References to architectural red lines in `CLAUDE.md`:

- **R3 Core Minimalism** — no heavy third-party libs (no container runtimes, no seccomp-bpf DSL, no gVisor)
- **R8 LLM Sovereignty** — Harness stays thin; no rule-engine intent detection, no multi-layer tool filter, no context-aggregation middleware
- **R10 Intelligence Lives in the Prompt** — migrated "intelligence" goes into system prompt templates, not new middleware
- **CLAUDE.md process-management warning** — two Aleph processes sharing `~/.aleph/data/` corrupts `.shared_token` → vault loss. This rules out Session-as-cross-process-daemon for v1.

## 5. Decisions Locked

| Axis | Option | Rationale |
|------|--------|-----------|
| Scope | Internal core reconstruction (replace `loop_core.rs` + `SessionManager`) | Full Anthropic semantic alignment; renames also cleanup |
| `AcpHarness` rename | `AcpAdapter` | Technically neutral; no collision with "agent" or "backend"; "adapter" accurately describes the role |
| Session flavor | In-process tokio actor behind `SessionService` trait, shape compatible with cross-process impl later | Achieves `wake(session_id)` + stateless harness without Aleph's multi-process vault risk |
| Sandbox flavor | `WorkspaceSandbox` (cwd + capability ledger + macOS seatbelt + `ExecGate`), trait shape permits process/container impls later | Respects R3; matches personal-assistant threat model; trait leaves upgrade path |
| Migration strategy | Strangler fig — new alongside old, per-phase shippable, legacy deleted in final phase | Required by "one phase = one brainstorm cycle = one shippable increment" constraint |

## 6. Cross-Cutting Principles

1. **Trait-first** — every new component is defined by a trait, impls are plug-in; Aleph's DI pattern (`AppContext` builder) stays.
2. **Event-sourced session** — every state change is an event appended to the session log; recovery = replay. No hidden in-memory state in Harness.
3. **Failure surfaces as tool events** — per Anthropic: harness / sandbox / tool failures all manifest to the LLM as tool-error events on the session log, not as Rust panics or control-flow exceptions.
4. **Shippable slices** — every phase ends with a green `just test-all`, no half-migrated state visible to users, and a `just release` possible.
5. **Observability preserved** — existing `stream.*` / `agent.*` / `session.*` event topics remain emitted across the migration; no observability regression.

## 7. Roadmap — 6 Phases

Each phase below is a **pointer to a future brainstorming cycle**. The bullets here define **scope + exit criteria only**. Detailed design happens in a subsequent `brainstorming → spec → plan → implement` cycle per phase.

### Phase 0 — Terminology Reset & Glossary

**Scope**:
- Rename `AcpHarness` → `AcpAdapter` across `src/acp/**` + all 16 presets + tests + docs
- Create `docs/reference/GLOSSARY.md` with Anthropic-aligned definitions of Harness / Sandbox / Orchestrator / Tools / Session — explicitly noting that the existing `SandboxManager` / `ExecSecurityGate` / `ApprovalGate` are **OS-level primitives**, distinct from the upcoming `Sandbox` trait (which is the agent-workspace abstraction)
- Update `ARCHITECTURE.md` + `AGENT_SYSTEM.md` terminology references to the new Harness semantics

**Non-scope**:
- No behavior changes; purely mechanical + documentation
- Do **not** rename `SandboxManager` in this phase — the rename decision lives naturally inside Phase 3 where the new `Sandbox` trait is introduced; touching it in Phase 0 risks pre-committing before the trait shape is known

**Exit criteria**:
- `grep -r 'AcpHarness' src/` returns zero hits
- `GLOSSARY.md` committed and cross-linked from `ARCHITECTURE.md`
- `cargo build && just test-all` green
- One release shipped

**Next brainstorm topic**: — (trivial; may not need its own brainstorm cycle; can be executed directly)

---

### Phase 1 — Session Service (Actor)

**Scope**:
- Define `SessionService` trait in `src/session/service.rs` with async methods (`get_events`, `emit_event`, `wake`, `snapshot`, etc.)
- Define `SessionEvent` enum — append-only event schema covering: turn started/ended, user message, assistant message, tool call requested, tool result, error, compaction, budget update
- Implement `InProcessActorSessionService` — one tokio task per session, mpsc command channel, watch channel for event stream, SQLite-backed persistence (reuse existing backend)
- Define `wake(session_id)` protocol — deterministic event replay to reconstruct in-flight turn state
- Wire `agent_loop/**` to use `SessionService` trait instead of direct `SessionManager` access (adapter keeps existing Gateway RPC methods working)
- Leave old `SessionManager` in place for Gateway-facing paths not yet migrated

**Exit criteria**:
- `SessionService` trait + in-process actor impl merged
- `agent_loop` module imports only `SessionService`, never `SessionManager`
- `wake(session_id)` demonstrable in a test: kill the in-flight turn mid-way, call `wake`, verify state recovery
- Event schema documented; `serde` round-trip tested
- Zero regression on existing `session.*` Gateway RPC surface

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-session-service-actor-design.md`

---

### Phase 2 — Tools Unified `execute()` Interface

**Scope**:
- Define `ToolService` façade in `src/tools/service.rs` with signature `execute(name: &str, input: serde_json::Value) -> Result<ToolOutput>`
- Unify dispatch across builtin / MCP / extension registries behind `ToolService`; internal `AlephTool` static trait stays for impls
- Migrate agent_loop tool pipeline to call `ToolService` only
- Define `ToolOutput` — standard shape with `value`, `error`, `metadata` (cost, latency, truncated flag)
- Tool schema discovery unified: `ToolService::list() → Vec<ToolDefinition>` covers all three registries

**Exit criteria**:
- `agent_loop` has zero direct imports of `McpClient`, `BuiltinToolRegistry`, `ExtensionTool`
- All tool calls go through `ToolService::execute`
- Hot-reload still works (adding/removing MCP server mid-session)
- Tool filters (`SmartFilter`, `ContextRule`) relocated as `ToolService` middleware layer — not middleware on the agent loop side

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-tool-service-unification-design.md`

---

### Phase 3 — Sandbox Trait (Workspace Impl)

**Scope**:
- Define `Sandbox` trait in `src/sandbox/mod.rs` with `execute(command: SandboxCommand) -> Result<SandboxOutput>`
- Define `SandboxCommand` — command + args + env + stdin + capability requirements
- Implement `WorkspaceSandbox` — cwd root + capability ledger + macOS seatbelt profile (drives into existing `SandboxManager`) + `ExecSecurityGate` + `ApprovalGate` flow reused verbatim
- Decide and execute the rename of the current `SandboxManager` (it's actually an OS-sandbox profile driver, not the agent-level Sandbox) — candidate names: `OsSandboxDriver`, `SeatbeltRunner`, or keep-as-is with docstring disambiguation. This is a sub-decision of Phase 3's brainstorm
- Exec-class tools (`bash_exec`, `file_write`, `file_delete`, etc.) routed through `Sandbox::execute` instead of directly spawning `std::process::Command`
- Non-exec tools (`memory_search`, `llm_call`, etc.) bypass Sandbox
- On-demand provisioning: `WorkspaceSandbox` lazily created on first exec-class tool call per session

**Exit criteria**:
- Exec-class tools no longer directly reference `std::process::Command` or sandbox-exec binaries
- `Sandbox` trait + `WorkspaceSandbox` impl green under existing exec tests
- Capability ledger + audit events continue flowing to `session.*` topics unchanged
- Clear trait seam documented for future `ProcessSandbox` impl
- `SandboxManager` either renamed (new name documented) or retained with disambiguating docstring

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-sandbox-trait-workspace-design.md`

---

### Phase 4 — Harness (Think→Act Loop)

**Scope**:
- New `src/harness/` module; `Harness::run_turn(session_id, deps) -> TurnOutcome`
- Dependencies injected: `SessionService`, `ToolService`, `Sandbox` factory, `LLMProvider`
- Harness is stateless — all state read/written via `SessionService` events
- Loop body: fetch pending events → call LLM → parse decision → route tool calls to `ToolService` (Sandbox under the hood for exec tools) → emit result events → loop-exit check
- All failure modes (LLM error, tool error, sandbox error, harness panic) captured as `SessionEvent::Error` on the log
- `wake(session_id)` behavior: if events show an in-flight turn, Harness resumes from the last committed event
- Feature flag `ALEPH_HARNESS_V2` toggles between old `loop_core` and new Harness for gradual rollout
- Old `loop_core.rs` kept; new Harness covers same functional surface first, then we cut over

**Exit criteria**:
- Feature-flagged: both paths pass the same integration test suite
- `ALEPH_HARNESS_V2=1` produces identical user-visible behavior on smoke tests
- `wake(session_id)` end-to-end test: crash Harness mid-turn, spawn new instance, recovery verified
- `loop_core.rs` marked as legacy in docstring

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-harness-think-act-design.md`

---

### Phase 5 — Orchestrator & Flow Composition

**Scope**:
- New `src/orchestrator/` module; owns session lifecycle + Harness dispatch + Sandbox provisioning + routing
- Define `FlowSpec` — declarative flow config: `{brain: BrainRef, tools_allowed: Vec<ToolName>, sandbox_kind: SandboxKind, session_strategy: SessionStrategy}`
- Gateway requests → Orchestrator resolves FlowSpec → spawns Harness with correct deps → streams events
- Built-in preset flows (`default-agent`, `code-reviewer`, `researcher`, ...) + user-defined flows via `aleph.toml`
- Existing `teams/`, `sub_agents/`, `groups/` refactored to spawn child flows via Orchestrator instead of custom plumbing
- R9 alignment: flow selection and composition remain tool-driven (LLM selects flow, not rule engine)

**Exit criteria**:
- Gateway agent request paths all routed via Orchestrator
- `FlowSpec` YAML/TOML schema documented + example flows shipped
- Multi-agent features (spawn/delegate/team) functionally equivalent via Orchestrator
- All three paths (agent.run single, subagent spawn, team collaborative session) share the same Orchestrator → Harness codepath

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-orchestrator-flow-composition-design.md`

---

### Phase 6 — Legacy Deletion & Stabilization

**Scope**:
- Delete `agent_loop/loop_core.rs` main loop; retain only types that moved elsewhere
- Delete `SessionManager` direct call sites; delete the feature flag `ALEPH_HARNESS_V2`
- Retire old shims introduced during strangler migration
- Final documentation pass: `ARCHITECTURE.md`, `AGENT_SYSTEM.md`, `MULTI_AGENT_SYSTEM.md` rewritten around new architecture
- Final performance + regression sweep

**Exit criteria**:
- `grep -rn 'loop_core\|SessionManager\|ALEPH_HARNESS_V2' src/` returns only grep-expected hits
- No duplicated agent-driver code paths
- Full `just test-all` + `just clippy` clean
- Final release shipped and tagged as "managed-agents architecture v1.0"

**Next brainstorm topic**: `docs/superpowers/specs/YYYY-MM-DD-managed-agents-stabilization-design.md`

## 8. Cross-Cutting Concerns (Non-Phase)

Addressed incrementally inside each phase rather than as dedicated phases:

- **Observability**: Every phase emits `session.*`, `agent.*`, `stream.*` events continuously; Phase 1 defines the canonical event schema that later phases extend.
- **Audit & Security**: `ExecSecurityGate` + `ApprovalGate` remain the audit chokepoints; `capability_ledger` events continue to flow. Phase 3 formalizes the audit contract on the `Sandbox` trait by having `WorkspaceSandbox::execute` call through the existing gates.
- **ACP traffic**: `AcpAdapter` (post-Phase-0 rename) stays a separate subsystem; it is a peer to the new Harness, not a consumer of it. Long-term: Orchestrator may route some ACP invocations as `BrainKind::AcpAdapter`, but not in v1.
- **Hot-reload of tools/flows**: Preserved throughout; Phase 2 keeps MCP hot-reload, Phase 5 extends to flow hot-reload.
- **Vault / shared-token safety**: No change — remains single-process owned; explicitly driving the decision to keep Session in-process.

## 9. Success Metrics (Whole Refactor)

- **Decoupling metric**: `agent_loop/loop_core.rs` deleted; new top-level modules have < 800 lines per file
- **Recovery metric**: Harness crash + `wake(session_id)` recovers in-flight turn in < 2s (measured)
- **Extensibility metric**: Adding a new flow takes < 50 LOC + 1 TOML entry; no changes to Harness/Orchestrator core
- **Regression metric**: All existing integration tests green throughout; zero user-visible behavior regression on `agent.run` / `session.*` / `memory.*` RPC methods
- **Test coverage metric**: Each new module (`session`, `tools::service`, `sandbox`, `harness`, `orchestrator`) ≥ 80% line coverage
- **Doc metric**: `GLOSSARY.md` + one design doc per phase, all cross-linked from `ARCHITECTURE.md`

## 10. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `wake(session_id)` replay produces non-deterministic state | Medium | High | Event schema includes all decision inputs (prompt, seed, tool args); replay is pure function of log; integration tests cover crash-recovery |
| Feature flag `ALEPH_HARNESS_V2` drift — two codepaths diverge | Medium | Medium | Shared integration test suite runs both paths; Phase 6 deletes flag before feature drift accumulates |
| Phase 2 tool unification breaks MCP hot-reload | Low | High | Hot-reload integration test added at Phase 2 gate |
| Phase 3 Sandbox refactor breaks macOS seatbelt edge cases | Medium | Medium | Keep existing seatbelt logic verbatim inside `WorkspaceSandbox`; only the call-site changes |
| Phase 5 Orchestrator pulls too many concerns, becomes the new 4000-line monolith | Medium | High | Orchestrator scope is narrow: session lifecycle + dispatch only; flow composition state lives on Session events |
| Roadmap invalidated by an upstream Anthropic model/API change mid-refactor | Low | Medium | Each phase is independently shippable; we can pause between phases without half-migrated state |
| Aleph release cadence (~daily CalVer) slowed by refactor | High | Low | Strangler + per-phase shipping means every phase ends green; daily releases continue |

## 11. Open Questions (Deferred to Per-Phase Brainstorms)

- Phase 1: Event schema versioning — do we commit to a stable on-disk event format from v1, or allow breaking changes pre-v1.0?
- Phase 1: Does `SessionService` support cross-session operations (e.g., search across all sessions) or is that strictly the memory subsystem's domain?
- Phase 2: Should MCP tool-call errors be retried inside `ToolService` or always bubble to Harness?
- Phase 3: Where does workspace cwd live on disk — `~/.aleph/workspaces/{session_id}/` per session, or shared?
- Phase 4: Does the new Harness support concurrent tool calls within one turn (current `CascadePolicy::Isolated`) on day one, or is that a follow-up?
- Phase 5: `FlowSpec` file format — TOML (consistent with `aleph.toml`) or YAML (more common in the agent ecosystem)?
- Phase 5: Can a flow be invoked by LLM tool call (`flow_run` tool)? If yes, flow composition becomes self-referential.

## 12. Not Now (Explicitly Deferred)

- Cross-process Session daemon (`Option C` from brainstorm)
- Process-isolated or container-based Sandbox (`Option B/C` from brainstorm)
- Multi-host Aleph deployments
- Replacing LLM provider layer
- Changing Gateway protocol / JSON-RPC surface
- Unifying `teams`, `sub_agents`, `groups` into a single primitive — Phase 5 unifies their **plumbing** only; semantic unification is a follow-up design effort.

## 13. Roadmap-Level Timeline

No calendar commitment. Phase ordering is strict (each builds on the previous). Expected shape:

```
Phase 0 ── Phase 1 ── Phase 2 ── Phase 3 ── Phase 4 ── Phase 5 ── Phase 6
(small)    (large)    (medium)   (medium)   (large)    (large)    (medium)
```

Parallelism opportunities:
- Phase 2 and Phase 3 are mostly independent; can overlap if two contributors available.
- Phase 0 can be executed directly without its own brainstorm cycle (trivial, mechanical).

## 14. Next Action

1. User reviews this roadmap document
2. Once approved, invoke `writing-plans` skill to produce a **plan file for Phase 0** (the only phase trivial enough to plan directly)
3. Phase 1 onward: each phase begins with its own `brainstorming` cycle, producing a per-phase design spec in `docs/superpowers/specs/`, then a per-phase plan, then implementation.
