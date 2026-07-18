# P6 — Checkpoint Retraction + Boot Assembly Doc Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land one commit on the long-lived `harness-dissolution` branch that creates `docs/reference/BOOT_ASSEMBLY.md` (12-module assembly map) and marks P6 retracted in the roadmap with footnote ⁶ explaining why `src/session/checkpoint/` and `src/runtime/boot.rs` are not happening.

**Architecture:** Two new/modified files in a single commit. (1) `docs/reference/BOOT_ASSEMBLY.md` — net-new ~220-line reference doc per spec D4 option β (§1–§5 of TOC, no §6 future-failures). (2) `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — 5 surgical Edits closing out P6. No source-code changes; visual `git diff` is the only verification gate.

**Tech Stack:** git, the Edit tool, the Write tool. Nothing else.

---

## Spec Reference

This plan implements `docs/superpowers/specs/2026-04-25-p6-checkpoint-boot-design.md` (committed at `e12d3464c`). See spec §2 (Audit Evidence) for why `checkpoint/` and `boot.rs` are retracted, §3 (D1–D9) for decisions, §3.3 (BOOT_ASSEMBLY.md outline) for the doc structure, §4 (Commit Plan) for the verification bar.

## Operating Constraints

- **Worktree**: All work happens in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`. Do NOT touch `/Volumes/TBU4/Workspace/Aleph` (main repo).
- **Branch**: `harness-dissolution` (long-lived, P0–P7). Do NOT push. Do NOT merge to main. User decides merge timing later.
- **Commit message**: Single-line `-m` only. Multi-line HEREDOC bodies trip `block-no-verify@1.1.2` hook false-positives.
- **Verification bar**: No source-code change → no cargo check / clippy required. Visual `git diff` review is the only gate.

---

## Task 1: Create `docs/reference/BOOT_ASSEMBLY.md`

**Files:**
- Create: `docs/reference/BOOT_ASSEMBLY.md` (~220 lines)

- [ ] **Step 1: Write the full doc**

Use the Write tool with `file_path` = `/Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/reference/BOOT_ASSEMBLY.md` and `content` = the literal markdown below (do not modify a single character — this is the agreed scope under spec D4 option β):

````markdown
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
````

- [ ] **Step 2: Confirm file landed at the expected size**

Run:
```
wc -l /Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/reference/BOOT_ASSEMBLY.md
```
Expected: ~210–230 lines (allow ±10 for fenced-block / blank-line variation).

If significantly outside this range, re-Read the file and diff against the literal block above.

---

## Task 2: Roadmap Close-Out

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (5 edits)

- [ ] **Step 1: Read the roadmap once**

`Read` `/Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` so subsequent Edits are allowed by the harness.

- [ ] **Step 2: Update §3.3 row 7 (Module 7 — State & Checkpointing)**

Use the Edit tool with `replace_all: false`.

old_string:
```
| 7 | State & Checkpointing | `src/session/` (+ `checkpoint/` submodule) | Fill Git-style checkpoint contracts |
```

new_string:
```
| 7 | State & Checkpointing | `src/session/` (kept in place)⁶ | `SessionEventStore` + `SessionActor::replay()` + `SessionState` projection already form a complete event-sourced replay framework; `checkpoint/` submodule + Git-style checkpoint contracts retracted (see note ⁶). |
```

- [ ] **Step 3: Update §3.3 row 12 (Module 12 — Initialization & Environment)**

Use the Edit tool with `replace_all: false`.

old_string:
```
| 12 | Initialization & Environment | `src/init_unified/` + `src/config/` + new `src/runtime/boot.rs` | Document 12-module assembly order |
```

new_string:
```
| 12 | Initialization & Environment | `src/init_unified/` + `src/config/` + `docs/reference/BOOT_ASSEMBLY.md`⁶ | 12-module assembly order documented in `docs/reference/BOOT_ASSEMBLY.md` (option β); proposed `src/runtime/boot.rs` retracted — actual runtime boot lives in `src/bin/aleph-server/commands/start/` (6,194 LOC) and relocating it is a separate refactor with no current consumer demand (see note ⁶). |
```

- [ ] **Step 4: Update §4.2 P6 row**

Use the Edit tool with `replace_all: false`.

old_string:
```
| **P6** | `P6-checkpoint-boot` | State checkpoint + boot assembly | 🟢 Low | 1 week | `src/session/checkpoint/`; `src/runtime/boot.rs` assembly order documented |
```

new_string:
```
| **P6** | `P6-checkpoint-boot` | State checkpoint + boot assembly | 🟢 Low⁶ | 1 day⁶ | `src/session/checkpoint/` retracted (event-sourced replay already covers it); `src/runtime/boot.rs` retracted (binary boot stays in place); 12-module assembly order documented in `docs/reference/BOOT_ASSEMBLY.md` (see note ⁶) |
```

- [ ] **Step 5: Update §7 status table P6 row**

Use the Edit tool with `replace_all: false`.

old_string:
```
| P6 | 📋 Planned | — | — | — | — |
```

new_string:
```
| P6 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p6-checkpoint-boot-design.md](./2026-04-25-p6-checkpoint-boot-design.md) | (no plan needed — see commit log) |
```

- [ ] **Step 6: Insert footnote ⁶ between footnote ⁵ and `### 4.3 Dependency Graph`**

Use the Edit tool with `replace_all: false`. The anchor is footnote ⁵'s full paragraph (one long paragraph) immediately followed by a blank line and then `### 4.3 Dependency Graph`.

old_string (footnote ⁵'s full paragraph + blank line + §4.3 heading — copy verbatim):
```
⁵ **P5 YAGNI retraction (2026-04-25)**: P5 brainstorm performed a full code census of the four directories the roadmap proposed to merge into `src/subagents/`: `src/agents/` (32 files / 17,503 LOC / 27 external consumers), `src/teams/` (18 files / 9,203 LOC / 19 external consumers), `src/orchestrator/` (21 files / 4,163 LOC / 4 external consumers), `src/group_chat/` (8 files / 5,770 LOC / 4 external consumers) — totaling 79 files / 36,639 LOC / 54 external consumer files. Findings: (a) all four directories are healthy live code with active production runtime paths (gateway/execution_engine, builtin_tools, providers, thinker, A2A, Telegram); (b) responsibilities are orthogonal — `agents/` = runtime, `teams/` = lifecycle/persistence, `orchestrator/` = flow composition, `group_chat/` = channel — collapsing them into one directory would dilute responsibility boundaries rather than clarify them; (c) two earlier dead-code claims were rebutted: every `teams/` trait has a `Sqlite*` implementation (textbook trait+impl dependency-inversion), and the 9 `unimplemented!()` in `agents/swarm/context_injector.rs:671-719` are intentional `#[cfg(test)] MockTaskStore` narrow stubs; (d) the proposed `SubagentOrchestrator` trait + Fork/Handoff/Graph modes have zero current consumers — each spawn path today (`builtin_tools/sessions/spawn_tool.rs`, `builtin_tools/team/delegate.rs`, `a2a/`, `orchestrator/`) calls a concrete API directly, not through a `dyn` trait — so adding a unifying trait would violate R3 (Core Minimalism), R8 (LLM Sovereignty), and R10 (Intelligence in Prompt). Conclusion: 4-way merge retracted, no `src/subagents/` directory created (avoiding the P0→P2 placeholder anti-pattern), `SubagentOrchestrator`/Fork/Handoff/Graph trait commitment retracted. The 4 directories stay where they are. If a future 5th subagent shape eventually demands a unified entry-point, a new phase named **P8-subagent-merge** (not "P5 round 2") should revisit consolidation. Risk downgraded 🔴 High → 🟢 Low; estimate shortened 3 weeks → 1 day. See P5 design §2–§3 for details.

### 4.3 Dependency Graph
```

new_string (same ⁵ paragraph, then blank line, then ⁶ paragraph, then blank line, then §4.3 heading):
```
⁵ **P5 YAGNI retraction (2026-04-25)**: P5 brainstorm performed a full code census of the four directories the roadmap proposed to merge into `src/subagents/`: `src/agents/` (32 files / 17,503 LOC / 27 external consumers), `src/teams/` (18 files / 9,203 LOC / 19 external consumers), `src/orchestrator/` (21 files / 4,163 LOC / 4 external consumers), `src/group_chat/` (8 files / 5,770 LOC / 4 external consumers) — totaling 79 files / 36,639 LOC / 54 external consumer files. Findings: (a) all four directories are healthy live code with active production runtime paths (gateway/execution_engine, builtin_tools, providers, thinker, A2A, Telegram); (b) responsibilities are orthogonal — `agents/` = runtime, `teams/` = lifecycle/persistence, `orchestrator/` = flow composition, `group_chat/` = channel — collapsing them into one directory would dilute responsibility boundaries rather than clarify them; (c) two earlier dead-code claims were rebutted: every `teams/` trait has a `Sqlite*` implementation (textbook trait+impl dependency-inversion), and the 9 `unimplemented!()` in `agents/swarm/context_injector.rs:671-719` are intentional `#[cfg(test)] MockTaskStore` narrow stubs; (d) the proposed `SubagentOrchestrator` trait + Fork/Handoff/Graph modes have zero current consumers — each spawn path today (`builtin_tools/sessions/spawn_tool.rs`, `builtin_tools/team/delegate.rs`, `a2a/`, `orchestrator/`) calls a concrete API directly, not through a `dyn` trait — so adding a unifying trait would violate R3 (Core Minimalism), R8 (LLM Sovereignty), and R10 (Intelligence in Prompt). Conclusion: 4-way merge retracted, no `src/subagents/` directory created (avoiding the P0→P2 placeholder anti-pattern), `SubagentOrchestrator`/Fork/Handoff/Graph trait commitment retracted. The 4 directories stay where they are. If a future 5th subagent shape eventually demands a unified entry-point, a new phase named **P8-subagent-merge** (not "P5 round 2") should revisit consolidation. Risk downgraded 🔴 High → 🟢 Low; estimate shortened 3 weeks → 1 day. See P5 design §2–§3 for details.

⁶ **P6 YAGNI retraction + doc-only investment (2026-04-25)**: P6 brainstorm audited the two roadmap-proposed deliverables — `src/session/checkpoint/` Git-style checkpoint contracts (module 7) and `src/runtime/boot.rs` 12-module assembly documentation (module 12). Findings: (a) the "Git-style checkpoint contracts" already exist in fact under different names — `SessionEventStore` trait (`src/session/store.rs`) + `SessionActor::replay()` (`src/session/actor.rs:69`) + `SessionState` pure projection (`src/session/state.rs:3`) form a complete event-sourced replay framework with test coverage (`replay_rebuilds_head_seq` at `actor.rs:230`, `replay_is_deterministic` at `state.rs:212`); adding a parallel `Checkpoint` trait beside this triple would duplicate semantics with zero present consumer; (b) actual runtime boot lives in `src/bin/aleph-server/commands/start/` totalling 6,194 LOC across 5 files (`mod.rs` 1,656 LOC, `builder/agent_init.rs` 1,905 LOC, `builder/handlers.rs` 1,951 LOC, `builder/subsystems.rs` 581 LOC, `orchestrator_init.rs` 94 LOC) — relocating it into a new `src/runtime/boot.rs` is a large refactor with no current consumer demand; (c) the stated outcome ("assembly order documented") is satisfied at lower cost by a single reference doc — `docs/reference/BOOT_ASSEMBLY.md` (~220 lines, option β: §1–§5 of TOC) — citing `file:line` rather than copy-pasting code so the doc survives modest refactors. Conclusion: `src/session/checkpoint/` retracted (parallel-abstraction zero-consumer pattern, same shape as P5's `SubagentOrchestrator`), `src/runtime/boot.rs` retracted (no consumer demand), no `src/runtime/` directory created (avoiding P0→P2 placeholder anti-pattern). Net change: 0 LOC of source code, 1 new reference doc (`BOOT_ASSEMBLY.md`). Risk unchanged 🟢 Low; estimate shortened 1 week → 1 day. See P6 design §2 for details.

### 4.3 Dependency Graph
```

---

## Task 3: Visual Diff Review + Commit

- [ ] **Step 1: Visual diff review of roadmap edits**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution diff docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

Expected: 5 distinct hunks corresponding to Steps 2–6 of Task 2. Read each hunk and confirm:
1. §3.3 row 7: Final Directory column now shows `` `src/session/` (kept in place)⁶ `` and Action column describes retraction
2. §3.3 row 12: Final Directory column now references `docs/reference/BOOT_ASSEMBLY.md`⁶ and Action column describes the retraction + new doc
3. §4.2 P6 row: Risk is `🟢 Low⁶`, Estimate is `1 day⁶`, Exit Artifact mentions `BOOT_ASSEMBLY.md`
4. §7 P6 row: status `✅ Complete` with both dates `2026-04-25` and a link to the design doc
5. New footnote ⁶ inserted between footnote ⁵ and `### 4.3 Dependency Graph`

- [ ] **Step 2: Confirm new BOOT_ASSEMBLY.md is present**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```

Expected output:
```
?? docs/reference/BOOT_ASSEMBLY.md
 M docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

(The leading `??` means untracked-new-file, ` M` means modified-tracked.)

- [ ] **Step 3: Stage both files and verify**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution add docs/reference/BOOT_ASSEMBLY.md docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```

Expected output:
```
A  docs/reference/BOOT_ASSEMBLY.md
M  docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

(Both staged: `A` for added, `M` for modified.)

- [ ] **Step 4: Commit**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution commit -m "docs: P6 — retract session checkpoint trait + add BOOT_ASSEMBLY.md doc"
```

Expected output: `[harness-dissolution <sha>] docs: P6 — retract session checkpoint trait + add BOOT_ASSEMBLY.md doc` with `2 files changed, ...`.

If the commit hook complains (`block-no-verify`, etc.), do NOT use `--no-verify`. Read the hook output, fix the underlying issue, and try again with a single-line `-m` message.

- [ ] **Step 5: Post-commit verification**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution log -1 --format='%H %s' && \
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short && \
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution show --stat HEAD
```

Expected:
- log line shows the new commit SHA + the exact commit message
- `git status --short` is empty (working tree clean)
- `git show --stat HEAD` shows exactly 2 files changed:
  - `docs/reference/BOOT_ASSEMBLY.md` — new file, ~210–230 insertions
  - `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — 6 insertions / 4 deletions (give or take)

---

## Verification Summary

After Tasks 1–3 complete:

- ✅ `docs/reference/BOOT_ASSEMBLY.md` exists with §1–§5 (option β) under ~220 lines
- ✅ Roadmap §3.3 row 7, §3.3 row 12, §4.2 P6 row, §7 P6 row updated
- ✅ Footnote ⁶ added to roadmap between ⁵ and `### 4.3 Dependency Graph`
- ✅ Single commit on `harness-dissolution`, single-line message, no push, no merge to main
- ✅ No source-code changes (no cargo check / clippy required)
- ✅ Working tree clean (`git status --short` empty after the commit)

---

## Rollback

If anything goes wrong after the commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-sha>
```
This restores the roadmap to its pre-commit state (P6 row still `📋 Planned`, no footnote ⁶) and deletes `BOOT_ASSEMBLY.md`. No source-code side effects.
