# Harness Dissolution Roadmap

**Date**: 2026-04-24
**Status**: Approved (brainstorm phase)
**Scope**: Architectural roadmap for dissolving `src/harness/` into the 12-module Agent Harness ontology across Aleph's codebase.

---

## 1. Background

### 1.1 The 12-Module Agent Harness Ontology

Per the industry consensus codified in early 2026 (Anthropic, OpenAI, LangChain, global AI engineering community), a production-grade Agent Harness consists of 12 independent but interlocked modules:

1. **Orchestration Loop** — the "dumb loop" (ReAct / TAO). Drives turn-by-turn dispatch; contains no reasoning itself.
2. **Tools** — the agent's hands. Standardized schema, registration, validation, sandboxed execution.
3. **Memory** — short-term (conversation) and long-term (cross-session persistence).
4. **Context Management** — combat "context rot" and "lost in the middle". Compression, masking, just-in-time retrieval, subagent delegation.
5. **Prompt Assembly** — layered stack: system → tools → memory → history → user.
6. **Tool Calling & Structured Output** — schema-constrained tool invocation; eliminate fuzzy parsing.
7. **State & Checkpointing** — resume, rollback, debug. Git-style checkpoints.
8. **Error Handling** — transient / model-recoverable / user-fixable / unexpected.
9. **Guardrails** — input, output, tool-call. Emergency brake.
10. **Verification & Feedback** — rule-based (linter, tests), visual (screenshot), LLM-as-judge.
11. **Subagent Orchestration** — fork, handoff, nested state graphs.
12. **Initialization & Environment Setup** — the agent lifecycle boot.

### 1.2 The Anthropic Thin-Harness Philosophy

> "Models get stronger → harness gets thinner."

The orchestration loop is deliberately "dumb". All intelligence lives in the model and the prompt. Harness is scaffolding, not a cognitive layer. This aligns with Aleph's architectural redlines **R3 (Core Minimalism)**, **R8 (LLM Sovereignty)**, and **R10 (Intelligence Lives in the Prompt)**.

### 1.3 Current State Problem

`src/harness/` today is a 16-file, 3712-line module that mixes the **orchestration core** with concerns that belong to other domains:

- Context management spills across 5 locations: `harness/context_budget/`, `harness/context_compactor.rs`, `src/context/`, `src/compressor/`, `src/memory/compaction/`.
- Prompt assembly is split three ways: `src/thinker/` (30+ files, the real workhorse), `src/prompt/` (mode selector), `src/harness/sections/` (markdown fragments).
- Guardrails have no unified entry: logic is scattered across `src/security/`, `src/sandbox/`, `src/permission/`, `src/approval/`, `src/pii/`.
- Subagent orchestration is split among 5 directories with overlapping intent: `agents/`, `teams/`, `orchestrator/`, `group_chat/`, `supervisor/`.
- `src/resilience/` has been gutted (per its own module doc: *"middleware have been removed"*) and now only holds `StateDatabase`.
- `src/thinker/` and `src/harness/` have overlapping responsibilities from prior refactors.

Anthropic's article warns this is the difference between "toy demo" and "production-grade agent" — the harness structure itself is the performance lever.

---

## 2. Architectural Decisions

### 2.1 Direction: Dissolution (not Consolidation)

`src/harness/` will **not** expand to own the 12 modules. Instead it dissolves back to a **thin orchestration core**, and the other 11 concerns return to (or consolidate at) their rightful domain directories.

**Rejected alternatives**:
- *Consolidation* (make `src/harness/` the "Agent OS" containing everything): violates R3, couples unrelated domains, creates a monolith.
- *Re-framing only* (document the 12 modules without moving code): leaves the carnage in place.

### 2.2 Core Boundary: Thin Core + Shared Infrastructure

`src/harness/` retains:
- The Think→Act driver (`agent.rs`)
- Cross-module orchestration plumbing that doesn't belong to any single domain: callbacks, traces, loop-level hooks, chain context, dependency injection container
- The `Harness` trait + `HarnessError`

`src/harness/` **does not** retain:
- Context budget / compaction (→ `src/context/`)
- Stop hooks / verification (→ `src/verification/`)
- Prompt sections / adapters (→ `src/prompt_assembly/`)
- Provider bridge (→ `src/providers/`)
- Tool execution context / summaries (→ `src/tools/`, `src/tool_output/`)
- Skill prefetch (→ `src/skill/`)

### 2.3 Trait Contracts Live With Domains

The 12 modules' `trait` definitions live in each domain directory, not in a central `harness/contracts/` folder. `HarnessDeps` is the injection container that wires concrete implementations into the orchestration loop at boot time.

Rationale: Rust idiom — traits belong next to their closest consumers or implementations; centralization creates action-at-a-distance.

---

## 3. Final Target Topology

### 3.1 `src/harness/` After Dissolution (9 files, ~1500 lines)

```
src/harness/
├── mod.rs                  # Exports Harness trait + AgentHarness
├── agent.rs                # Think→Act loop (slimmed, <500 lines)
├── deps.rs                 # HarnessDeps: DI container
├── trait_def.rs            # Harness trait + HarnessError
├── callback.rs             # HarnessCallback (orchestration events)
├── loop_callback.rs        # LoopCallback (turn-level hooks)
├── trace.rs                # Orchestration trace collection
├── trace_sink.rs           # Trace output abstraction
└── chain_context.rs        # Turn-to-turn state plumbing
```

### 3.2 Relocation Manifest (current `src/harness/` → target)

| Current | Target | Reason |
|---------|--------|--------|
| `harness/context_budget/` (8 files) | `src/context/budget/` | Context engineering, not orchestration |
| `harness/context_compactor.rs` | `src/context/compact/` (merged with `src/compressor/`) | One place for all compaction |
| `harness/stop_hooks.rs` | `src/verification/stop_hooks.rs` | Stop decisions are verification (module 10) |
| `harness/verify_stop_hook.rs` | `src/verification/` | Same |
| `harness/skill_prefetch.rs` | `src/skill/prefetch.rs` | Skill subsystem owns this |
| `harness/sections/` (markdown + guidance) | `src/prompt_assembly/sections/` | Prompt source material |
| `harness/adapters/` | `src/tools/adapters/` | Tool-source bridges (BuiltinToolAdapter, McpToolAdapter, DaemonQueryTool, MemoryStoreTool, registry builders) — confirmed during P0 brainstorm |
| `harness/provider_bridge.rs` | `src/providers/bridge.rs` | Provider protocol adapter |
| `harness/tool_execution_context.rs` | `src/tools/execution_context.rs` | Tool domain |
| `harness/tool_summary.rs` | `src/tool_output/summary.rs` | Existing tool_output absorbs it |

### 3.3 12-Module Final Home Map

| # | Module | Final Directory | Action |
|---|--------|-----------------|--------|
| 1 | Orchestration Loop | `src/harness/` | **Retain** (slim) |
| 2 | Tools | `src/tools/` + `src/builtin_tools/` | Absorb harness exec_context |
| 3 | Memory | `src/memory/` | No change |
| 4 | Context Management | **`src/context/{budget,compact,window}/`** | **Merge 5 locations** |
| 5 | Prompt Assembly | `src/thinker/` (kept in place)⁴ | Dead code in `prompt`/`payload`/`capability`/`prompt_assembly` deleted; 3-way merge and `PromptAssembler` trait retracted (see note ⁴) |
| 6 | Tool Calling / Structured Output | `src/tools/calling/` | Absorb harness/provider_bridge split-out |
| 7 | State & Checkpointing | `src/session/` (kept in place)⁶ | `SessionEventStore` + `SessionActor::replay()` + `SessionState` projection already form a complete event-sourced replay framework; `checkpoint/` submodule + Git-style checkpoint contracts retracted (see note ⁶). |
| 8 | Error Handling | Cross-module (`HarnessError` + typed errors)⁷ | **Split** (revised in P0 brainstorm): (a) rename `src/resilient/` → `src/task_resilience/` — lands in P0; (b) `src/resilience/` retained as-is⁷ — audit revealed StateDatabase is a legitimate single-connection multi-tenant ApplicationRecord pattern (5,031 LOC active code, 28 consumers), `gutted ⇒ deletable` premise was factually wrong; rename/relocate adds mechanical churn with zero architectural value (see note ⁷). |
| 9 | Guardrails | `src/{security,sandbox,approval,pii}/` (kept in place)³ | Orphan `src/permission/` deleted; facade and InputGuard/OutputGuard/ToolCallGuard traits retracted (see note ³) |
| 10 | Verification & Feedback | **`src/verification/`** (new) | Absorb stop_hooks; add rule/visual/LLM-judge |
| 11 | Subagent Orchestration | `src/{agents,teams,orchestrator,group_chat}/` (kept in place)⁵ | All 4 directories are healthy live code with orthogonal responsibilities; 4-way merge and `SubagentOrchestrator` trait retracted (see note ⁵). `supervisor/` → `src/process_supervisor/` rename already landed in P0. |
| 12 | Initialization & Environment | `src/init_unified/` + `src/config/` + `docs/reference/BOOT_ASSEMBLY.md`⁶ | 12-module assembly order documented in `docs/reference/BOOT_ASSEMBLY.md` (option β); proposed `src/runtime/boot.rs` retracted — actual runtime boot lives in `src/bin/aleph-server/commands/start/` (6,194 LOC) and relocating it is a separate refactor with no current consumer demand (see note ⁶). |

---

## 4. Roadmap: 7 Phases

### 4.1 Design Principles

1. **Physical moves first**: File relocation + import renaming is cheap and makes the map readable.
2. **Consolidate before adding**: Eliminate fragmentation before introducing new capabilities.
3. **Risk last**: Subagent orchestration (5-way collision) is the most dangerous — ship it last.
4. **Every phase ships independently**: Aleph must run at the end of each phase; no "half-dissolved" states allowed.
5. **Trait contracts emerge with their domain**: Each phase establishes the traits for its module — no separate "contracts phase".

### 4.2 Phase List

| Phase | Code | Theme | Risk | Estimate | Exit Artifact |
|-------|------|-------|------|----------|---------------|
| **P0** | `P0-slim-harness` | Harness physical slimming | 🟢 Low | 1 week | `src/harness/` down to 9 files; supervisor renamed; `resilience/` deleted |
| **P1** | `P1-context-engine` | Context engineering consolidation | 🟢 Low¹ | 3–5 days¹ | `src/context/{budget,compact}/` unified (see note ¹) |
| **P2** | `P2-prompt-assembly` | Prompt assembly consolidation | 🟢 Low⁴ | 1 day⁴ | `prompt`/`payload`/`capability`/`prompt_assembly` deleted (~5,200 LOC); facade trait plan retracted (see note ⁴) |
| **P3** | `P3-guardrails` | Guardrails facade | 🟢 Low³ | 1–2 hours³ | Orphan `src/permission/` deleted; facade plan retracted (see note ³) |
| **P4** | `P4-verification` | Verification & feedback loop | 🟢 Low² | 1–2 hours² | `src/verification/` houses StopHookHandler + ShellStopHook only (see note ²) |
| **P5** | `P5-subagents` | Subagent orchestration collapse | 🟢 Low⁵ | 1 day⁵ | 4-way merge retracted; `agents`/`teams`/`orchestrator`/`group_chat` confirmed healthy live code with orthogonal responsibilities; `SubagentOrchestrator` trait + Fork/Handoff/Graph modes retracted (see note ⁵) |
| **P6** | `P6-checkpoint-boot` | State checkpoint + boot assembly | 🟢 Low⁶ | 1 day⁶ | `src/session/checkpoint/` retracted (event-sourced replay already covers it); `src/runtime/boot.rs` retracted (binary boot stays in place); 12-module assembly order documented in `docs/reference/BOOT_ASSEMBLY.md` (see note ⁶) |
| **P7** | `P7-state-layer` | State layer reorganization (added 2026-04-24) | 🟢 Low⁷ | 1 day⁷ | `src/resilience/` retained as-is (StateDatabase is a healthy ApplicationRecord pattern, 5,031 LOC + 28 consumers); rename/relocate retracted (mechanical churn with zero architectural value); current state documented in `docs/reference/STATE_LAYER.md` (see note ⁷) |

**Total**: ~13.5 weeks / ~3.5 months.

¹ **P1 YAGNI downscoping (2026-04-24)**: During P1 brainstorm, the `ContextEngine` trait and `src/context/window/` subdirectory were explicitly deferred. The existing `CompactionStrategy` trait already provides the pluggable surface, and "window" concerns remain distributed across `ContextBudgetConfig` and `CompactorConfig` fields without sufficient mass to justify a standalone module. Additionally, `src/compressor/` was deleted (confirmed dead code, zero consumers) rather than "merged" as originally framed in §3.2. Risk downgraded from 🟡 Medium to 🟢 Low; estimate shortened from 2 weeks to 3–5 days. See P1 design §2 Decision 3 and §9 for rationale.

² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4 brainstorm, the roadmap's "rule / visual / LLM-judge contracts" commitment was retracted. Aleph's verification logic lives entirely in prompt templates (see `src/thinker/layers/agent_role.rs` VERDICT block) per R8/R10; no Rust-level verifier trait has a present consumer. A separate finding: `VerifyStopHook` (194 lines in `src/verification/verify_stop_hook.rs`) was orphaned code — zero production instantiations since its April 2026 introduction in commit b54877d7f — and was deleted per the P1 compressor precedent (dead code with zero consumers gets removed, not renamed). Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P4 design §2–§4 for details.

³ **P3 YAGNI retraction + orphan deletion (2026-04-25)**: P3 brainstorm audited the five modules originally proposed for the guardrails facade. Findings: (a) `src/permission/` was orphan code — zero external consumers since its April 2026 introduction in commit `1f7b33931` — and was deleted per the P1 (`compressor`) / P4 (`VerifyStopHook`) precedent (dead code with zero consumers gets removed, not relocated); (b) the four live modules (`security`, `sandbox`, `approval`, `pii`) serve genuinely distinct domains with distinct consumer footprints, so a parent `src/guardrails/` directory was rejected as adding hierarchy without solving any pain; (c) the planned `InputGuard` / `OutputGuard` / `ToolCallGuard` traits had no present consumer and were retracted (R3 + YAGNI). A separate fragmentation finding — three parallel exec-approval implementations (`src/exec/approval/`, `src/sandbox/exec_approval/`, `src/tools/middleware/permission/`) and six distinct `ApprovalDecision` types across the codebase — is layered/domain-distinct rather than a name collision, and is deferred to a future phase. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P3 design §2–§3 for details.

⁴ **P2 YAGNI retraction (2026-04-25)**: P2 brainstorm audited the three locations the roadmap proposed to merge (`src/thinker/`, `src/prompt/`, `src/harness/sections/`) plus two adjacent modules surfaced during the audit (`src/payload/`, `src/capability/`). Findings: (a) `src/prompt/` (747 lines) was dead — its sole consumer chain `payload::assembler::intent` was itself dead, and it was built on the already-removed `UnifiedIntentClassifier`; (b) `src/payload/` (~1,700 lines) and `src/capability/` (~2,500 lines) form a closed loop of test-only code — `PromptAssembler::build_prompt_with_intent_result` and `CapabilitySystem::execute()` are only called from their own unit tests; (c) `src/prompt_assembly/` (289 lines) was orphan scaffolding created in P0 awaiting P2 wiring, with zero external consumers; (d) `src/harness/sections/` was already moved during P0 (see §3.2). All four were deleted per the P1 (`compressor`)/P3 (`permission`)/P4 (`VerifyStopHook`) precedent (~5,200 LOC removed). The `PromptAssembler` / `Section` / `PromptLayer` trait commitment was retracted: `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction, and adding a parallel trait layer violates R3 (Core Minimalism) + R8 (LLM Sovereignty) + R10 (Intelligence in Prompt). The thinker→prompt_assembly directory rename (open question 2) was also retracted: a pure naming refactor touching 30 files + 200+ imports is not worth the diff cost. `src/thinker/` stays as the canonical prompt assembly home under its historical name. `src/intent/` (~150 lines, type-only) stays for gateway slash-command metadata serialization. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 2 weeks → 1 day. See P2 design §2–§3 for details.

⁵ **P5 YAGNI retraction (2026-04-25)**: P5 brainstorm performed a full code census of the four directories the roadmap proposed to merge into `src/subagents/`: `src/agents/` (32 files / 17,503 LOC / 27 external consumers), `src/teams/` (18 files / 9,203 LOC / 19 external consumers), `src/orchestrator/` (21 files / 4,163 LOC / 4 external consumers), `src/group_chat/` (8 files / 5,770 LOC / 4 external consumers) — totaling 79 files / 36,639 LOC / 54 external consumer files. Findings: (a) all four directories are healthy live code with active production runtime paths (gateway/execution_engine, builtin_tools, providers, thinker, A2A, Telegram); (b) responsibilities are orthogonal — `agents/` = runtime, `teams/` = lifecycle/persistence, `orchestrator/` = flow composition, `group_chat/` = channel — collapsing them into one directory would dilute responsibility boundaries rather than clarify them; (c) two earlier dead-code claims were rebutted: every `teams/` trait has a `Sqlite*` implementation (textbook trait+impl dependency-inversion), and the 9 `unimplemented!()` in `agents/swarm/context_injector.rs:671-719` are intentional `#[cfg(test)] MockTaskStore` narrow stubs; (d) the proposed `SubagentOrchestrator` trait + Fork/Handoff/Graph modes have zero current consumers — each spawn path today (`builtin_tools/sessions/spawn_tool.rs`, `builtin_tools/team/delegate.rs`, `a2a/`, `orchestrator/`) calls a concrete API directly, not through a `dyn` trait — so adding a unifying trait would violate R3 (Core Minimalism), R8 (LLM Sovereignty), and R10 (Intelligence in Prompt). Conclusion: 4-way merge retracted, no `src/subagents/` directory created (avoiding the P0→P2 placeholder anti-pattern), `SubagentOrchestrator`/Fork/Handoff/Graph trait commitment retracted. The 4 directories stay where they are. If a future 5th subagent shape eventually demands a unified entry-point, a new phase named **P8-subagent-merge** (not "P5 round 2") should revisit consolidation. Risk downgraded 🔴 High → 🟢 Low; estimate shortened 3 weeks → 1 day. See P5 design §2–§3 for details.

⁶ **P6 YAGNI retraction + doc-only investment (2026-04-25)**: P6 brainstorm audited the two roadmap-proposed deliverables — `src/session/checkpoint/` Git-style checkpoint contracts (module 7) and `src/runtime/boot.rs` 12-module assembly documentation (module 12). Findings: (a) the "Git-style checkpoint contracts" already exist in fact under different names — `SessionEventStore` trait (`src/session/store.rs`) + `SessionActor::replay()` (`src/session/actor.rs:69`) + `SessionState` pure projection (`src/session/state.rs:3`) form a complete event-sourced replay framework with test coverage (`replay_rebuilds_head_seq` at `actor.rs:230`, `replay_is_deterministic` at `state.rs:212`); adding a parallel `Checkpoint` trait beside this triple would duplicate semantics with zero present consumer; (b) actual runtime boot lives in `src/bin/aleph-server/commands/start/` totalling 6,194 LOC across 5 files (`mod.rs` 1,656 LOC, `builder/agent_init.rs` 1,905 LOC, `builder/handlers.rs` 1,951 LOC, `builder/subsystems.rs` 581 LOC, `orchestrator_init.rs` 94 LOC) — relocating it into a new `src/runtime/boot.rs` is a large refactor with no current consumer demand; (c) the stated outcome ("assembly order documented") is satisfied at lower cost by a single reference doc — `docs/reference/BOOT_ASSEMBLY.md` (~220 lines, option β: §1–§5 of TOC) — citing `file:line` rather than copy-pasting code so the doc survives modest refactors. Conclusion: `src/session/checkpoint/` retracted (parallel-abstraction zero-consumer pattern, same shape as P5's `SubagentOrchestrator`), `src/runtime/boot.rs` retracted (no consumer demand), no `src/runtime/` directory created (avoiding P0→P2 placeholder anti-pattern). Net change: 0 LOC of source code, 1 new reference doc (`BOOT_ASSEMBLY.md`). Risk unchanged 🟢 Low; estimate shortened 1 week → 1 day. See P6 design §2 for details.

⁷ **P7 YAGNI retraction + doc-only investment (2026-04-25)**: P7 brainstorm audited the three roadmap-proposed deliverables for `src/resilience/`: (i) "decide StateDatabase home (merge into `src/session/` or new `src/state/`)", (ii) "delete gutted `src/resilience/`", (iii) "20+ consumers updated". Findings: (a) `src/resilience/` is **not gutted** — it contains 5,031 LOC of live code across 13 files (`types.rs` 682 LOC + `database/` 4,334 LOC across 12 files); the mod.rs comment "Only the database layer (StateDatabase) and shared types remain" is true but "remain ⇒ deletable" was the roadmap's false premise; (b) StateDatabase is a legitimate **single-connection multi-tenant SQLite Repository** (ApplicationRecord pattern) — 1 × `Arc<Mutex<Connection>>` shared across 11 private submodules (`events.rs`, `memory_events.rs`, `tasks.rs`, `sessions.rs`, `group_chat.rs`, `traces.rs`, `paired_users.rs`, `channel_offsets.rs`, `replay.rs`, `state_database/schema.rs`, sticker_descriptions); each submodule attaches methods via `impl StateDatabase { ... }`; only `pub mod migration` is exposed publicly; 28 consumer files all `use crate::resilience::StateDatabase` with **zero imports of `crate::resilience::database::events` or any other submodule path** — encapsulation is leak-free; (c) candidate homes both fail audit: `src/session/` already owns `SessionEventStore`/`SessionActor::replay()`/`SessionState` (P6) and absorbing 11 non-session domains' storage breaks high-cohesion; `src/state/` is a brand-new directory equivalent to a pure rename with zero architectural value; (d) splitting StateDatabase across domains (path B) would convert strong encapsulation into weak encapsulation, expand consumer import surface (`use ::EventsStore` + `use ::TasksStore` + …), and introduce SQLite multi-writer lock contention on a single DB file; (e) `types.rs` types are de-facto crate vocabulary (RiskLevel 24 consumers, Lane 10, AgentEvent 10, TaskStatus 9, SessionStatus 9) — physical migration cost vastly exceeds benefit. Conclusion: rename/relocate retracted (mechanical churn, zero architectural value, same shape as P5's `SubagentOrchestrator` and P6's `checkpoint/`); split retracted (anti-pattern); deletion retracted (false premise). Net change: 0 LOC of source code, 1 new reference doc (`docs/reference/STATE_LAYER.md`, ~120 lines, option β: §1–§5 of TOC) capturing the misleading-name + ApplicationRecord + 28-consumer + cross-domain-types facts so future engineers don't re-propose the same retracted refactor. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1 day. See P7 design §2–§3 for details.

### 4.3 Dependency Graph

```
P0 (slim) ──┬─→ P1 (context-engine) ────┐
            ├─→ P2 (prompt-assembly) ───┤
            │                           ├─→ P5 (subagents) ──→ P6 (checkpoint-boot)
            ├─→ P3 (guardrails)  ───────┤
            └─→ P4 (verification) ──────┘
                  ↑
                  └── P3 and P4 may run in parallel
```

**Critical dependencies**:
- P0 unlocks everything else — stable boundaries are a prerequisite.
- P1–P4 are mutually independent; can run in parallel or serially based on team bandwidth.
- P5 **must be last** among substantive work: subagent orchestration will consume the traits defined by context-engine, prompt-assembly, guardrails, and verification. Running it earlier means wading through swamp.
- P6 is closing polish; depends only on P5.

### 4.4 Per-Phase Standard Lifecycle

Each phase is a full independent cycle (new brainstorm session, not a continuation):

1. **Brainstorm** (this skill) — refine target state, trait contracts, edge cases.
2. **Spec** — write to `docs/superpowers/specs/YYYY-MM-DD-phaseN-<name>-design.md`.
3. **Plan** (`writing-plans` skill) — decompose into tasks with acceptance criteria.
4. **Impl** (`subagent-driven-development` / `executing-plans`) — execute the plan.
5. **Close** — update this roadmap with phase completion + lessons learned.

### 4.5 Universal Phase Exit Criteria

- `cargo check -p alephcore` passes.
- `just test-all` passes.
- New / modified traits have unit test coverage.
- All old files touched by the phase are **deleted or fully migrated** — no dual residency allowed.
- This roadmap document is updated with the phase's completion status.

---

## 5. Out of Scope (YAGNI)

- ❌ **Do not rewrite `src/memory/`** — it is already the cleanest of the 12 modules.
- ❌ **Do not rewrite `src/tools/` body** — only relocate execution_context and summary.
- ❌ **Do not redesign the Think→Act loop itself** — the thin driver is already adequate; this refactor reshapes *what's around* the loop, not the loop.
- ❌ **Do not design a "Harness framework" for external consumers** — violates R3. Aleph's harness is internal scaffolding, not a product surface.
- ❌ **Do not touch desktop-bridge / interface layers** — orthogonal to this work.

---

## 6. Open Questions

These do not block this roadmap but will need resolution during each phase's brainstorm:

1. **P1** ✅ **Resolved (2026-04-24)**: The original framing was inaccurate. `src/memory/compaction/` in fact held the live-conversation compaction framework (PressureLevel, CompactionStrategy trait, Orchestrator, etc.) rather than offline memory-note refinement. P1 relocated the framework to `src/context/compact/` and moved the one truly cross-session component (`session_summary_source`) into `src/memory/session_compactor/summary_source.rs`. The `src/memory/compaction/` directory no longer exists. See P1 design §2 Decision 1.
2. **P2** ✅ **Resolved (2026-04-25)**: P2 brainstorm chose to keep `src/thinker/` under its historical name. The `thinker` → `prompt_assembly` rename was retracted as a pure naming refactor touching 30 files + 200+ import sites with no behavioral payoff. The `src/prompt_assembly/` scaffold (created in P0 awaiting P2 wiring) was deleted as orphan code per the P1/P3/P4 retraction precedent. See P2 design §1 (Anti-goals) and footnote ⁴.
3. **P5** ✅ **Resolved (2026-04-24, by P0)**: `src/supervisor/` was confirmed to be PTY subprocess supervision and was renamed to `src/process_supervisor/` during P0 (commit `4f96f2d66`-era work). The 4-way merge of the remaining subagent directories was subsequently retracted during P5 brainstorm — see footnote ⁵.
4. **P4**: `HarnessError` currently lives in `src/harness/trait_def.rs`. Does it stay there, or move to a top-level `src/error/harness.rs`? Cross-cutting error types are awkward either way.

---

## 7. Status Tracking

| Phase | Status | Started | Completed | Spec | Plan |
|-------|--------|---------|-----------|------|------|
| P0 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p0-slim-harness-design.md](./2026-04-24-p0-slim-harness-design.md) | [2026-04-24-p0-slim-harness.md](../plans/2026-04-24-p0-slim-harness.md) |
| P1 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p1-context-management-design.md](./2026-04-24-p1-context-management-design.md) | [2026-04-24-p1-context-management.md](../plans/2026-04-24-p1-context-management.md) |
| P2 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p2-prompt-assembly-design.md](./2026-04-25-p2-prompt-assembly-design.md) | (no plan needed — see commit log) |
| P3 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p3-guardrails-design.md](./2026-04-25-p3-guardrails-design.md) | [2026-04-25-p3-guardrails.md](../plans/2026-04-25-p3-guardrails.md) |
| P4 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p4-verification-design.md](./2026-04-24-p4-verification-design.md) | [2026-04-24-p4-verification.md](../plans/2026-04-24-p4-verification.md) |
| P5 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p5-subagents-design.md](./2026-04-25-p5-subagents-design.md) | (no plan needed — see commit log) |
| P6 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p6-checkpoint-boot-design.md](./2026-04-25-p6-checkpoint-boot-design.md) | (no plan needed — see commit log) |
| P7 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p7-state-layer-design.md](./2026-04-25-p7-state-layer-design.md) | (no plan needed — see commit log) |

Legend: 📋 Planned · 🚧 In Progress · ✅ Complete · ⏸️ On Hold

---

## 8. References

- `/Volumes/TBU4/Agent-Harness.md` — source article (12-module framework, Chinese).
- Anthropic Managed Agents engineering post — https://www.anthropic.com/engineering/managed-agents
- `CLAUDE.md` architectural redlines R3, R8, R10 — thin core, LLM sovereignty, intelligence in prompt.
- Current harness refactor phases 4b–7 — see `git log -- src/harness/`.
- Previous spec: `docs/superpowers/specs/2026-04-19-harness-think-act-design.md` (the Think→Act driver this roadmap builds on).
