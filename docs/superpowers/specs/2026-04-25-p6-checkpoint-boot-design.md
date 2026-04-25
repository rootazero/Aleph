# P6 — Checkpoint Retraction + Boot Assembly Doc Design

**Date**: 2026-04-25
**Phase**: P6 (`P6-checkpoint-boot`)
**Worktree**: `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`
**Branch**: `harness-dissolution` (long-lived, P0–P7)
**Status**: Spec ready for review

---

## 1. Goals & Anti-Goals

### Goals (3)

1. Retract roadmap §3.3 row 7's "fill Git-style checkpoint contracts" plan — `src/session/checkpoint/` submodule will not be created.
2. Replace roadmap §3.3 row 12's `src/runtime/boot.rs` plan with a doc-only artifact: `docs/reference/BOOT_ASSEMBLY.md` (~220 lines) describing the 12-module assembly order.
3. Close the P6 row in the roadmap (§4.2 + §3.3 + §7) with footnote ⁶ explaining the retraction and the substitution.

### Anti-Goals (5)

1. **No `src/session/checkpoint/` submodule.** `SessionEventStore` + `SessionActor::replay()` + `SessionState` already form a complete event-sourced replay framework with test coverage. Adding a parallel "checkpoint" trait/struct duplicates existing semantics with zero present consumer.
2. **No "Git-style checkpoint" trait.** The append-only `session_events` log + deterministic projection is already isomorphic to git's commit-DAG + checkout-and-rebuild — adding a Rust-level trait named `Checkpoint` parallel to `SessionEvent` would violate R3 (Core Minimalism) and R8 (LLM Sovereignty).
3. **No `src/runtime/boot.rs` code file.** The actual boot pathway lives in `src/bin/aleph-server/commands/start/` (6,194 LOC across 5 files); relocating it into `src/runtime/` is a large refactor with no current consumer demand.
4. **No `src/runtime/` directory.** Avoid the P0→P2 placeholder anti-pattern (creating empty scaffolding in anticipation of code that may never arrive).
5. **No source-code changes.** P6 produces exactly two file changes: one new reference doc (`docs/reference/BOOT_ASSEMBLY.md`) and one updated roadmap (`docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`).

---

## 2. Audit Evidence (Code Census)

### 2.1 Sub-Module 7 — Session Checkpoint: Retraction Evidence

The roadmap proposes "fill Git-style checkpoint contracts" inside a new `src/session/checkpoint/` directory. A full audit of `src/session/` (14 files) shows the contracts are already present under different names:

| Already-implemented component | Location | Test |
|------------------------------|----------|------|
| `SessionEventStore` trait — append-only persistence interface | `src/session/store.rs` | unit tests in same file |
| `SqliteEventStore` impl — rusqlite backend | `src/session/store.rs` | covered by integration tests |
| `SessionActor::replay()` — startup replay from SQLite | `src/session/actor.rs:69` | `replay_rebuilds_head_seq` (`actor.rs:230`) |
| `SessionState` — pure-function projection of the event stream | `src/session/state.rs:3` | `replay_is_deterministic` (`state.rs:212`) |
| Crash-recovery flow — "spawn fresh actor; it replays from SQLite" | `src/session/in_process.rs:154` | production path |
| Turn-boundary replay semantics | `src/harness/agent.rs:515` | production path |

**Conclusion**: an event-sourced log + deterministic projection is the canonical "Git-style" model — append-only events ≅ commit DAG; replay ≅ `git reset --hard HEAD` followed by replay; turn-boundary trim ≅ `git revert`. Adding a parallel `Checkpoint` trait beside the existing `SessionEventStore`+`SessionActor`+`SessionState` triple would duplicate semantics with zero consumer. **Same shape as P5's `SubagentOrchestrator` retraction**: zero-consumer parallel abstraction.

### 2.2 Sub-Module 12 — Boot Assembly: Doc-Only Substitution Evidence

The roadmap proposes `src/runtime/boot.rs` to "document 12-module assembly order". A boot-pathway audit reveals two separate concerns currently exist and a single-file substitution wouldn't capture either:

**Current runtime boot location** (binary, not lib):
```
src/bin/aleph-server/commands/start/
├── mod.rs                      1,656 LOC  # main entry, top-level wiring
├── orchestrator_init.rs           94 LOC  # multi-agent orchestrator subsystem
├── builder/
│   ├── mod.rs                      7 LOC  # re-exports
│   ├── agent_init.rs           1,905 LOC  # AgentRegistry + providers + tools
│   ├── handlers.rs             1,951 LOC  # gateway HTTP routes
│   └── subsystems.rs             581 LOC  # memory / sandbox / approval / pii / etc.
total                            6,194 LOC
```

**Current first-time setup location** (`src/init_unified/`):
- `coordinator.rs` runs a 5-phase install: Directories → Config → Database → Runtimes → Skills.
- This is **not** runtime boot — it runs only during first-time install or recovery, idempotent on subsequent starts.
- Re-exported through `src/lib.rs:99`.

**Position conflict**: relocating 6,194 LOC of binary boot code into `src/runtime/boot.rs` (lib) is a large refactor with no current consumer demand. The roadmap's stated outcome ("assembly order documented") is a *documentation* deliverable, not a code one — a `.md` file in `docs/reference/` solves the stated need at 0 LOC of code change.

**Real pain**: 3 files >1.6k LOC each — onboarding engineers can't quickly answer "in what order do the 12 modules from §3.3 wake up?". A single reference doc cross-linking module name → instantiation site → dependency arrow eliminates that pain at one-time, low-risk cost.

**Doc-only solution**: write `docs/reference/BOOT_ASSEMBLY.md` (~220 lines) — one entry per module from §3.3, citing `file:line` rather than copy-pasting code, so the doc survives modest refactors. See §3.3 below for the outline.

---

## 3. Key Decisions

### D1. Retract `src/session/checkpoint/` entirely

`SessionEventStore` + `SessionActor::replay()` + `SessionState` projection is the de facto checkpoint contract. No parallel abstraction.

### D2. Retract `src/runtime/boot.rs` (and `src/runtime/` directory)

No empty-directory placeholder. If a future phase ever needs runtime-only utilities outside `aleph-server/`, that phase brainstorms its own home — P6 doesn't pre-stake `src/runtime/`.

### D3. Add `docs/reference/BOOT_ASSEMBLY.md` instead

Single new reference doc. ~220 lines. Pure markdown — no diagrams unless plain-text-renderable. Links by `file:line`.

### D4. BOOT_ASSEMBLY.md scope: option β

Cover §1–§5 of the proposed TOC (see §3.3 of this spec):
- §1 Two distinct assembly phases (first-time setup vs. runtime boot)
- §2 First-time setup: `init_unified` 5-phase walk-through
- §3 Runtime boot: `aleph-server/commands/start/` entry-point map
- §4 12-module assembly order (one entry per module from roadmap §3.3)
- §5 Cross-module invariants (only invariants that have caused real bugs — speculative invariants excluded per YAGNI)

Excluded (option γ would have added):
- §6 "Common boot failures" — speculative future bugs; add when first real bug surfaces, not in advance.

### D5. Single commit, single-line message

```
docs: P6 — retract session checkpoint trait + add BOOT_ASSEMBLY.md doc
```

Multi-line HEREDOC trips the `block-no-verify@1.1.2` hook (P0–P5 lesson).

### D6. Verification bar inherits P5

No source-code change → no `cargo check`, no `cargo clippy`. Visual `git diff` is the only verification gate.

### D7. Footnote ⁶ pattern follows ⁴ + ⁵

Same opener (`⁶ **P6 ...**`), same `(a) … (b) … (c) …` findings lettering, same `Risk downgraded X → Y; estimate shortened M → N.` closing. Closing pointer: `See P6 design §2 for details.` (P6 doesn't have a §3 worth pointing at separately.)

### D8. §6 (Open Questions) unchanged

P6 introduces no new open questions. The existing question 4 (P4's `HarnessError` placement) is a P4 concern, not P6 — leave it untouched.

### D9. §3.3 dual annotation

Both row 7 (State & Checkpointing) and row 12 (Initialization & Environment) get a ⁶ marker — one footnote covers both.

---

## 3.3 BOOT_ASSEMBLY.md Outline (committed alongside)

The new reference doc, written under option β (D4):

```markdown
# Boot Assembly Reference

> Living map of how Aleph wires the 12 modules at startup. Read this when
> debugging boot failures, adding a new subsystem, or onboarding to the
> binary entry-point. As of 2026-04-25.

## 1. Two Distinct Assembly Phases
   Distinguish first-time setup (idempotent install wizard) from runtime
   boot (every-start wiring). They run in different binaries, persist
   different state, and fail in different ways.

## 2. First-Time Setup — `src/init_unified/`
   - 5-phase sequence: Directories → Config → Database → Runtimes → Skills
   - Entry: `init_unified/coordinator.rs:InitializationCoordinator`
   - Re-export site: `src/lib.rs:99`
   - When triggered: idempotent; runs on every start but each phase is a
     no-op if its target state is already present.

## 3. Runtime Boot — `src/bin/aleph-server/commands/start/`
   - Entry: `mod.rs::start_command` (1,656 LOC — the orchestrator)
   - Sub-builders:
     - `builder/subsystems.rs` — memory / sandbox / approval / pii / etc.
     - `builder/agent_init.rs` — providers, agents, tools (1,905 LOC)
     - `builder/handlers.rs` — gateway HTTP route registration (1,951 LOC)
     - `orchestrator_init.rs` — multi-agent orchestrator subsystem
   - Each sub-builder returns a typed handle the orchestrator combines.

## 4. 12-Module Assembly Order
   For each module in roadmap §3.3, one entry naming:
   - Module home directory
   - Instantiation site (file:line)
   - Direct dependencies (modules that must be ready first)

   Modules covered: 1 (Orchestration Loop), 2 (Tools), 3 (Memory),
   4 (Context), 5 (Prompt Assembly), 6 (Tool Calling), 7 (State/Session),
   8 (Error Handling), 9 (Guardrails), 10 (Verification),
   11 (Subagents), 12 (Initialization).

## 5. Cross-Module Invariants
   Only invariants that have caused real bugs in development:
   - Database connection must open before any *Store impl is constructed.
   - Provider registry must populate before AgentRegistry instantiation.
   - Skill loader must finish before tool registry finalization.
   (Future invariants: add only after the first real bug surfaces.)

## 6. References
   - Roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` §3.3
   - Top-level module declarations: `src/lib.rs`
```

The actual file expanded to ~220 lines lands in the implementation phase (the writing-plans skill produces the plan). This outline is the contract.

---

## 4. Commit Plan

### Single commit on `harness-dissolution`

**Files:**
- New: `docs/reference/BOOT_ASSEMBLY.md` (~220 lines)
- Modified: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (6 edits)

**Roadmap edits**:

| # | Section | Change |
|---|---------|--------|
| 1 | §3.3 row 7 (Module 7 — State & Checkpointing) | Action column: replace "Fill Git-style checkpoint contracts" with retraction note + ⁶ marker |
| 2 | §3.3 row 12 (Module 12 — Initialization & Environment) | Action column: replace `src/runtime/boot.rs` reference with `docs/reference/BOOT_ASSEMBLY.md` reference + ⁶ marker |
| 3 | §4.2 P6 row | Risk `🟢 Low` → `🟢 Low⁶`; Estimate `1 week` → `1 day⁶`; Exit Artifact rewritten |
| 4 | §7 P6 row | `📋 Planned \| — \| — \| — \| —` → `✅ Complete \| 2026-04-25 \| 2026-04-25 \| [link] \| (no plan needed)` |
| 5 | New footnote ⁶ | Inserted between footnote ⁵ and `### 4.3 Dependency Graph` |

**Commit message** (single line):
```
docs: P6 — retract session checkpoint trait + add BOOT_ASSEMBLY.md doc
```

**Verification bar**: visual `git diff` only. No `cargo check`, no `cargo clippy` (no source-code change).

**No push, no merge to main.** User decides merge timing after P6 lands.

---

## 5. Risk & Rollback

### Risk Profile: 🟢 Low

| Dimension | Assessment |
|-----------|------------|
| Source-code changes | 0 LOC |
| Trait / abstraction introductions | 0 |
| Build/test impact | 0 |
| Consumer impact | 0 (reference doc is read-only) |
| Roadmap self-consistency | requires footnote ⁶ in style consistent with ¹/²/³/⁴/⁵ |
| BOOT_ASSEMBLY.md accuracy | **only material risk** — citing wrong `file:line` would mislead. Mitigation: each citation is grep-verified during writing; doc is timestamped "as of 2026-04-25" so readers know to re-verify on stale entries. |

### Rollback

```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-sha>
```

Restores roadmap to pre-P6 state and deletes `BOOT_ASSEMBLY.md`. No source-code side effects.

### Comparison with prior phases

| Phase | Pattern | LOC delta | New design doc | New reference doc | Retracted items |
|-------|---------|-----------|----------------|-------------------|----------------|
| P1 | downscope + delete dead | ~−700 | 1 | 0 | `compressor` + `ContextEngine` trait |
| P2 | retract + delete dead | −5,200 | 1 | 0 | 4 dirs + `PromptAssembler`/`Section`/`PromptLayer` traits |
| P3 | retract + delete dead | ~−1,000 | 1 | 0 | `permission` + `InputGuard`/`OutputGuard`/`ToolCallGuard` traits |
| P4 | retract + delete dead | −194 | 1 | 0 | `VerifyStopHook` + `Verifier` trait |
| P5 | retract (doc-only) | 0 | 1 | 0 | 4-way merge + `SubagentOrchestrator` + Fork/Handoff/Graph modes |
| **P6** | retract + doc-only | **0** | **1** | **1** (BOOT_ASSEMBLY.md) | `session/checkpoint/` + `runtime/boot.rs` |

P6 is the first phase to net-add a reference doc; still zero source-code change.

---

## 6. References

- Parent roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
- Prior YAGNI-retraction phases: P1 (`2026-04-24-p1-context-management-design.md`), P2 (`2026-04-25-p2-prompt-assembly-design.md`), P3 (`2026-04-25-p3-guardrails-design.md`), P4 (`2026-04-24-p4-verification-design.md`), P5 (`2026-04-25-p5-subagents-design.md`)
- Architectural redlines: `CLAUDE.md` R3 (Core Minimalism), R8 (LLM Sovereignty), R10 (Intelligence in Prompt)

