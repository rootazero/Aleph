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
| 5 | Prompt Assembly | **`src/prompt_assembly/`** | **Merge 3 locations** (thinker + prompt + harness/sections) |
| 6 | Tool Calling / Structured Output | `src/tools/calling/` | Absorb harness/provider_bridge split-out |
| 7 | State & Checkpointing | `src/session/` (+ `checkpoint/` submodule) | Fill Git-style checkpoint contracts |
| 8 | Error Handling | Cross-module (`HarnessError` + typed errors) | **Split** (revised in P0 brainstorm): (a) rename `src/resilient/` → `src/task_resilience/` — lands in P0; (b) `src/resilience/` StateDatabase relocation — deferred to new phase **P7** (architectural decision, 20+ consumers) |
| 9 | Guardrails | **`src/guardrails/`** (new facade) | Aggregate security/sandbox/permission/approval/pii; InputGuard/OutputGuard/ToolCallGuard |
| 10 | Verification & Feedback | **`src/verification/`** (new) | Absorb stop_hooks; add rule/visual/LLM-judge |
| 11 | Subagent Orchestration | **`src/subagents/`** (new home) | **Collapse 4 dirs** (agents + teams + orchestrator + group_chat) + rename `supervisor/` → `src/process_supervisor/` (not subagent) |
| 12 | Initialization & Environment | `src/init_unified/` + `src/config/` + new `src/runtime/boot.rs` | Document 12-module assembly order |

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
| **P1** | `P1-context-engine` | Context engineering consolidation | 🟡 Medium | 2 weeks | `src/context/{budget,compact,window}/` unified; `ContextEngine` trait |
| **P2** | `P2-prompt-assembly` | Prompt assembly consolidation | 🟡 Medium | 2 weeks | 3-way merge → `src/prompt_assembly/`; `PromptAssembler` trait |
| **P3** | `P3-guardrails` | Guardrails facade | 🟡 Medium | 1.5 weeks | `src/guardrails/` with InputGuard/OutputGuard/ToolCallGuard; delegates to existing backing stores |
| **P4** | `P4-verification` | Verification & feedback loop | 🟡 Medium | 1.5 weeks | `src/verification/` absorbs stop_hooks; rule / visual / LLM-judge contracts |
| **P5** | `P5-subagents` | Subagent orchestration collapse | 🔴 High | 3 weeks | `src/subagents/` from 4-way merge (agents + teams + orchestrator + group_chat); `supervisor/` renamed out; `SubagentOrchestrator` trait; Fork / Handoff / Graph modes explicit |
| **P6** | `P6-checkpoint-boot` | State checkpoint + boot assembly | 🟢 Low | 1 week | `src/session/checkpoint/`; `src/runtime/boot.rs` assembly order documented |
| **P7** | `P7-state-layer` | State layer reorganization (added 2026-04-24) | 🟡 Medium | 1.5 weeks | Decide StateDatabase home (merge into `src/session/` or new `src/state/`); delete gutted `src/resilience/`; 20+ consumers updated |

**Total**: ~13.5 weeks / ~3.5 months.

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

1. **P1**: How should the boundary between `src/memory/compaction/` (within-memory consolidation) and `src/context/compact/` (cross-turn compression) be drawn? The memory compaction is about offline memory → note refinement; the context compaction is about live conversation trimming. They should stay separate but need a shared trait vocabulary.
2. **P2**: Should the unified prompt directory be named `src/prompt_assembly/` or keep `src/thinker/` (historical name)? `thinker` is misleading under R8 (thinking belongs to the LLM, not the assembler), so `prompt_assembly` is preferred, but it's a rename of ~30 files.
3. **P5**: Of the 5 subagent directories, `src/supervisor/` seems to be PTY subprocess supervision (unrelated to subagent orchestration). Confirm this in P5 brainstorm and relocate to `src/process_supervisor/`.
4. **P4**: `HarnessError` currently lives in `src/harness/trait_def.rs`. Does it stay there, or move to a top-level `src/error/harness.rs`? Cross-cutting error types are awkward either way.

---

## 7. Status Tracking

| Phase | Status | Started | Completed | Spec | Plan |
|-------|--------|---------|-----------|------|------|
| P0 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p0-slim-harness-design.md](./2026-04-24-p0-slim-harness-design.md) | [2026-04-24-p0-slim-harness.md](../plans/2026-04-24-p0-slim-harness.md) |
| P1 | 📋 Planned | — | — | — | — |
| P2 | 📋 Planned | — | — | — | — |
| P3 | 📋 Planned | — | — | — | — |
| P4 | 📋 Planned | — | — | — | — |
| P5 | 📋 Planned | — | — | — | — |
| P6 | 📋 Planned | — | — | — | — |
| P7 | 📋 Planned | — | — | — | — |

Legend: 📋 Planned · 🚧 In Progress · ✅ Complete · ⏸️ On Hold

---

## 8. References

- `/Volumes/TBU4/Agent-Harness.md` — source article (12-module framework, Chinese).
- Anthropic Managed Agents engineering post — https://www.anthropic.com/engineering/managed-agents
- `CLAUDE.md` architectural redlines R3, R8, R10 — thin core, LLM sovereignty, intelligence in prompt.
- Current harness refactor phases 4b–7 — see `git log -- src/harness/`.
- Previous spec: `docs/superpowers/specs/2026-04-19-harness-think-act-design.md` (the Think→Act driver this roadmap builds on).
