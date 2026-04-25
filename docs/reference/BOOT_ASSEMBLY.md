# Boot Assembly Reference

> Living map of how Aleph wires the 12 modules at startup. Read this when
> debugging boot failures, adding a new subsystem, or onboarding to the
> binary entry-point. As of 2026-04-25.

## 1. Two Distinct Assembly Phases

Aleph has two assembly pathways that run at different times and persist
different state:

| Phase | When it runs | Where | What it does |
|-------|--------------|-------|--------------|
| **First-Time Setup** | Once per environment (idempotent on re-runs) | `src/init_unified/` | Creates directories, generates default config, opens databases, downloads runtimes, installs default skills |
| **Runtime Boot** | Every server start | `src/bin/aleph-server/commands/start/` | Wires the 12 modules into a running gateway server |

The two are decoupled: first-time setup is a no-op when its target state
already exists, so runtime boot can assume it. Failures in first-time
setup do not corrupt runtime boot — they leave Aleph in an "uninstalled"
state and exit before the gateway opens.

## 2. First-Time Setup — `src/init_unified/`

5-phase sequence executed by `InitializationCoordinator`:

1. **Directories** — create `~/.aleph/{config,data,cache,logs}/` etc.
2. **Config** — generate `~/.aleph/config/aleph.toml` with defaults
3. **Database** — open SQLite databases under `~/.aleph/data/`
4. **Runtimes** — download Python / Node / etc. for skill execution
5. **Skills** — install default skills bundled with the binary

**Entry**: `src/init_unified/coordinator.rs:InitializationCoordinator`
**Re-export site**: `src/lib.rs:99`
**Idempotency**: each phase no-ops when its target state is already
present, so this coordinator is safe to call on every start (current
binary does call it).

## 3. Runtime Boot — `src/bin/aleph-server/commands/start/`

Total: 6,194 LOC across 5 files.

**Entry point**: `mod.rs:378 — start_server` orchestrates the full boot.

**Sub-builders** (each isolated to one responsibility):

| File | LOC | Responsibility |
|------|-----|----------------|
| `mod.rs` | 1,656 | Top-level orchestrator: tracing, config load, session store, extension manager, graceful shutdown wiring |
| `builder/subsystems.rs` | 581 | Auth (`initialize_auth`), device store, channel registration, inbound routing |
| `builder/agent_init.rs` | 1,905 | `AgentHandlersResult`: provider registry, agent registry, tool registry, execution engine, embedder, compression service, multi-provider registry |
| `builder/handlers.rs` | 1,951 | Gateway HTTP route registration |
| `orchestrator_init.rs` | 94 | Multi-agent orchestrator subsystem |

Each sub-builder returns a typed bundle (e.g. `AuthBundle`,
`AgentHandlersResult`) the orchestrator combines into the final
`GatewayServer`.

## 4. 12-Module Assembly Order

The 12 modules from roadmap §3.3, in the order they become ready during
runtime boot. Each entry names the module's home, the primary
instantiation site, and any dependency that must already be ready.

### Module 1 — Orchestration Loop
- **Home**: `src/harness/`
- **Primary instantiation**: per-session — created lazily by `ExecutionEngine` when an agent run is requested. Not assembled at boot time.
- **Dependencies**: tools registry (Module 2), context (Module 4), prompt assembly (Module 5), state (Module 7) all ready before first run.

### Module 2 — Tools
- **Home**: `src/tools/` + `src/builtin_tools/`
- **Primary instantiation**: `agent_init.rs::AgentHandlersResult.tool_registry` (`BuiltinToolRegistry`) — populated during agent handler registration.
- **Dependencies**: none for the registry itself; individual tools may need provider registry ready before invocation.

### Module 3 — Memory
- **Home**: `src/memory/`
- **Primary instantiation**: `agent_init.rs::AgentHandlersResult.embedder` + `compression_service` — created alongside agent registry; concrete `MemoryStore` opened when first agent loads memory.
- **Dependencies**: database (init_unified phase 3) must be ready.

### Module 4 — Context Management
- **Home**: `src/context/{budget,compact}/`
- **Primary instantiation**: per-session — `ContextBudget` and `CompactionStrategy` constructed by harness when assembling a turn.
- **Dependencies**: tools registry (Module 2), memory (Module 3), prompt assembly (Module 5).

### Module 5 — Prompt Assembly
- **Home**: `src/thinker/`
- **Primary instantiation**: per-session — `Thinker` constructed by harness when running a turn.
- **Dependencies**: provider registry (in `AgentHandlersResult.default_provider`), tools registry (Module 2), context (Module 4).

### Module 6 — Tool Calling / Structured Output
- **Home**: `src/tools/calling/`
- **Primary instantiation**: per-call — invoked from `harness/agent.rs` during a turn.
- **Dependencies**: tools registry (Module 2) populated.

### Module 7 — State & Session
- **Home**: `src/session/`
- **Primary instantiation**: `mod.rs:168 — initialize_session_store`. Builds `SqliteEventStore` and constructs `InProcessActorSessionService` via `mod.rs:249 — build_sqlite_session_service`.
- **Dependencies**: database (init_unified phase 3) must be ready.

### Module 8 — Error Handling
- **Home**: cross-module (`HarnessError` in `src/harness/trait_def.rs` + typed errors elsewhere)
- **Primary instantiation**: passive — error types are constructed at error-site, not assembled.
- **Dependencies**: none.

### Module 9 — Guardrails
- **Home**: `src/{security,sandbox,approval,pii}/`
- **Primary instantiation**: `subsystems.rs:38 — initialize_auth` constructs `AuthBundle` (TokenManager, PairingManager, InvitationManager, GuestSessionManager). Sandbox / approval / PII assembled per-tool-call inside execution engine.
- **Dependencies**: device store (created within `initialize_auth`), config (init_unified phase 2).

### Module 10 — Verification & Feedback
- **Home**: `src/verification/`
- **Primary instantiation**: per-session — `StopHookHandler` registered with execution engine when an agent run starts.
- **Dependencies**: harness (Module 1) running.

### Module 11 — Subagent Orchestration
- **Home**: `src/{agents,teams,orchestrator,group_chat}/` (kept separate per P5 retraction)
- **Primary instantiation**:
  - `AgentRegistry` — `agent_init.rs::AgentHandlersResult.agent_registry`
  - Team store — `agent_init.rs::AgentHandlersResult.team_store`
  - Orchestrator — `orchestrator_init.rs` (94 LOC)
  - GroupChat — channel-side, registered by `subsystems.rs` channel registration
- **Dependencies**: provider registry (within `AgentHandlersResult`), tools registry (Module 2).

### Module 12 — Initialization & Environment
- **Home**: `src/init_unified/` + `src/config/`
- **Primary instantiation**: `init_unified/coordinator.rs:InitializationCoordinator`. Re-exported at `src/lib.rs:99`.
- **Dependencies**: filesystem only (creates everything else).

## 5. Cross-Module Invariants

Only invariants whose violation has caused real bugs in development.
Speculative invariants are excluded — add new ones here only after the
first real bug surfaces.

### Invariant 1: Database open before any *Store impl is constructed
`SqliteEventStore`, `SqliteTeamStore`, `SqliteCoordTaskStore`, etc. all
require an open `rusqlite::Connection`. If init_unified phase 3
(Database) has not run, every store constructor panics at first call.
Mitigation: the `initialize_session_store` ordering in `mod.rs:168` is
sequenced after init_unified.

### Invariant 2: Provider registry populated before AgentRegistry
`AgentRegistry` resolves each agent's `provider_id` to a concrete
`AiProvider` at agent-load time. If the provider registry is empty,
agents register but cannot run — surfacing as a confusing "provider not
found" error mid-turn rather than at boot. Mitigation: `default_provider`
is threaded through `AgentHandlersResult` so the registration order is
enforced at the type level.

### Invariant 3: Skill loader finishes before tool registry finalization
Skills register additional tools into `BuiltinToolRegistry` during their
load. If `BuiltinToolRegistry` is sealed before init_unified phase 5
(Skills) completes, skill-provided tools are silently absent — no error,
just missing capabilities. Mitigation: the registry's "no more tools"
sentinel is deferred until after the install coordinator returns.

## 6. References

- Roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` §3.3
- Top-level module declarations: `src/lib.rs`
- Binary entry: `src/bin/aleph-server/main.rs` → `commands/start/mod.rs:378 — start_server`
