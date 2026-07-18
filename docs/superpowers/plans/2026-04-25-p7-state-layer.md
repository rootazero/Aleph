# P7 — State Layer Retraction + STATE_LAYER.md Doc Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land one commit on the long-lived `harness-dissolution` branch that creates `docs/reference/STATE_LAYER.md` (~120-line ApplicationRecord-pattern reference doc) and marks P7 retracted in the roadmap with footnote ⁷ explaining why `src/resilience/` is retained as-is.

**Architecture:** Two new/modified files in a single commit. (1) `docs/reference/STATE_LAYER.md` — net-new ~120-line reference doc per spec D6 option β (§1–§5 of TOC, no §6 future-failures). (2) `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — 4 surgical Edits closing out P7. No source-code changes; visual `git diff` is the only verification gate.

**Tech Stack:** git, the Edit tool, the Write tool. Nothing else.

---

## Spec Reference

This plan implements `docs/superpowers/specs/2026-04-25-p7-state-layer-design.md` (committed at `f1cb10793`). See spec §2 (Audit Evidence) for why `src/resilience/` is retained, §3 (D1–D9) for decisions, §3 D6 (STATE_LAYER.md outline) for the doc structure, §4 (Commit Plan) for the verification bar.

## Operating Constraints

- **Worktree**: All work happens in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`. Do NOT touch `/Volumes/TBU4/Workspace/Aleph` (main repo).
- **Branch**: `harness-dissolution` (long-lived, P0–P7). Do NOT push. Do NOT merge to main. User decides merge timing later.
- **Commit message**: Single-line `-m` only. Multi-line HEREDOC bodies trip `block-no-verify@1.1.2` hook false-positives.
- **Verification bar**: No source-code change → no cargo check / clippy required. Visual `git diff` review is the only gate.

---

## Task 1: Create `docs/reference/STATE_LAYER.md`

**Files:**
- Create: `docs/reference/STATE_LAYER.md` (~120 lines)

- [ ] **Step 1: Write the full doc**

Use the Write tool with `file_path` = `/Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/reference/STATE_LAYER.md` and `content` = the literal markdown below (do not modify a single character — this is the agreed scope under spec D6 option β):

````markdown
# State Layer Reference

> Living description of how `src/resilience/` actually works as of 2026-04-25.
> Read this before proposing to rename, relocate, or split the module.

## 1. Misleading Name, Living Code

The directory `src/resilience/` no longer hosts any "resilience" middleware.
Its self-description in `src/resilience/mod.rs:1-5` says:

```
//! Resilience Module — Database and Core Types
//!
//! Governance, collaboration, perception, and recovery middleware have been
//! removed as part of the agent loop migration. Only the database layer
//! (StateDatabase) and shared types remain.
```

This is true (governance / collaboration / perception / recovery middleware
were deleted by the P0 agent-loop dissolution) but **misleading**: "Only
… remain" suggests leftover deletable code, while the actual remainder is
**5,031 LOC of live code** powering 28 consumer files across 8 top-level
domains. The roadmap's original P7 exit-artifact "delete gutted
`src/resilience/`" was retracted on 2026-04-25 (see roadmap footnote ⁷).

The directory name is preserved for now — renaming would touch 28 consumers'
`use` statements without solving any current problem.

## 2. ApplicationRecord Pattern

`StateDatabase` (defined at `src/resilience/database/state_database/mod.rs:20`)
is a single-connection multi-tenant SQLite Repository — the Rust equivalent of
Rails' `ApplicationRecord` or Django's shared `Connection`.

**Shape:**

- 1 × `Arc<Mutex<Connection>>` — owned by the struct, shared across all
  domains via interior mutability
- Registers the `sqlite-vec` extension at construction time
  (`DEFAULT_EMBEDDING_DIM = 1024`)
- 6 public methods on the struct itself (`new`, `in_memory`, `new_with_dim`,
  `serialize_embedding`, `store_sticker_description`,
  `load_sticker_description`)
- 11 private `mod xxx` submodules each attach methods to the same struct
  via `impl StateDatabase { ... }`:

| Submodule | LOC | Domain |
|---|---|---|
| `events.rs` | 245 | Agent events |
| `memory_events.rs` | 635 | Memory event sourcing |
| `tasks.rs` | 233 | Agent tasks |
| `sessions.rs` | 211 | Sessions |
| `group_chat.rs` | 177 | Group chat sessions |
| `traces.rs` | 292 | Task traces (Shadow Replay) |
| `paired_users.rs` | 60 | Telegram paired users |
| `channel_offsets.rs` | 53 | Telegram offsets |
| `replay.rs` | 18 | Replay helper |
| `state_database/schema.rs` | 426 | DDL |
| sticker_descriptions (in `state_database/mod.rs`) | — | Sticker storage |

The only **public** submodule is `pub mod migration` (defined in
`src/resilience/database/migration.rs`, 626 LOC) — it owns the three
`add_*` migrations (channel_offsets, paired_users, sticker_descriptions)
called once during binary boot.

**Encapsulation is leak-free:** all 28 consumer files import
`crate::resilience::StateDatabase` (the pub re-export from
`src/resilience/mod.rs:15`). A `grep` for
`use crate::resilience::database::events` (or any other submodule path)
returns **zero matches**. The God-object surface is the boundary; the
internal partitioning is invisible to callers.

**Why this shape is correct:**

- SQLite best practice: a single `Arc<Mutex<Connection>>` avoids
  multi-writer lock contention on the same DB file
  (`~/.aleph/data/state.db`)
- Strong encapsulation: callers cannot bypass the struct to reach
  individual tables
- Refactor-friendly: adding a new domain = add a new private `mod`,
  attach `impl` methods, no consumer change

## 3. Consumer Distribution (28 files / 8 domains)

| Domain | File count | % | Notable consumers |
|---|---|---|---|
| `gateway/` | 9 | 33% | execution_engine, handlers, Telegram interface |
| `memory/` | 7 | 25% | events, integration tests, transcript_indexer |
| `executor/` | 4 | 14% | builtin_registry (builder/config/definitions/registry) |
| `bin/aleph-server/` | 3 | 11% | start/builder agent_init + handlers + subsystems |
| `group_chat/` | 2 | 7% | executor, orchestrator |
| `arena/` | 1 | 4% | storage |
| `lib.rs` | 1 | 4% | crate-level re-export |
| `sync_primitives.rs` | 1 | 4% | shared `Arc`/`Mutex` helper |

**Total: 28 distinct files**, all entering through the single pub type.

## 4. Cross-Domain Types (`src/resilience/types.rs`, 682 LOC)

The `types.rs` module hosts crate-wide vocabulary types. Their consumer
counts (number of distinct files using each type, excluding
`src/resilience/` itself):

| Type | Consumers | Use |
|---|---|---|
| `RiskLevel` | 24 files | Task risk classification (most cross-domain) |
| `Lane` | 10 files | Task execution lane |
| `AgentEvent` | 10 files | Agent event persistence record |
| `TaskStatus` | 9 files | Task status enum (Pending/Running/Completed/Failed/Interrupted) |
| `SessionStatus` | 9 files | Session lifecycle status |
| `SubagentSession` | 3 files | Long-lived subagent session record |
| `AgentTask` | 2 files | Task struct (state + recovery checkpoint) |
| `TaskTrace` | 1 file | Near-dead-code (single consumer) |

These are **de facto crate-wide vocabulary**: physically migrating them
into a different directory would force 24+ consumer-file edits per type
with zero architectural improvement.

## 5. References

- Roadmap: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
  §3.3 row 8 + §4.2 P7 row + footnote ⁷
- P7 design: `docs/superpowers/specs/2026-04-25-p7-state-layer-design.md`
- StateDatabase struct: `src/resilience/database/state_database/mod.rs:20`
- Misleading mod.rs comment: `src/resilience/mod.rs:1-5`
- Pub re-export: `src/resilience/mod.rs:15`
- Migration entry: `src/resilience/database/migration.rs`
- Cross-domain types: `src/resilience/types.rs`
````

- [ ] **Step 2: Confirm file landed at the expected size**

Run:
```
wc -l /Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/reference/STATE_LAYER.md
```
Expected: ~110–135 lines (allow ±15 for fenced-block / blank-line variation; the embedded literal block above is ~120 lines including blank lines).

If significantly outside this range, re-Read the file and diff against the literal block above.

---

## Task 2: Roadmap Close-Out

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (4 edits)

- [ ] **Step 1: Read the roadmap once**

`Read` `/Volumes/TBU4/Workspace/Aleph.harness-dissolution/docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` so subsequent Edits are allowed by the harness.

- [ ] **Step 2: Update §3.3 row 8 (Module 8 — Error Handling)**

Use the Edit tool with `replace_all: false`.

old_string:
```
| 8 | Error Handling | Cross-module (`HarnessError` + typed errors) | **Split** (revised in P0 brainstorm): (a) rename `src/resilient/` → `src/task_resilience/` — lands in P0; (b) `src/resilience/` StateDatabase relocation — deferred to new phase **P7** (architectural decision, 20+ consumers) |
```

new_string:
```
| 8 | Error Handling | Cross-module (`HarnessError` + typed errors)⁷ | **Split** (revised in P0 brainstorm): (a) rename `src/resilient/` → `src/task_resilience/` — lands in P0; (b) `src/resilience/` retained as-is⁷ — audit revealed StateDatabase is a legitimate single-connection multi-tenant ApplicationRecord pattern (5,031 LOC active code, 28 consumers), `gutted ⇒ deletable` premise was factually wrong; rename/relocate adds mechanical churn with zero architectural value (see note ⁷). |
```

- [ ] **Step 3: Update §4.2 P7 row**

Use the Edit tool with `replace_all: false`.

old_string:
```
| **P7** | `P7-state-layer` | State layer reorganization (added 2026-04-24) | 🟡 Medium | 1.5 weeks | Decide StateDatabase home (merge into `src/session/` or new `src/state/`); delete gutted `src/resilience/`; 20+ consumers updated |
```

new_string:
```
| **P7** | `P7-state-layer` | State layer reorganization (added 2026-04-24) | 🟢 Low⁷ | 1 day⁷ | `src/resilience/` retained as-is (StateDatabase is a healthy ApplicationRecord pattern, 5,031 LOC + 28 consumers); rename/relocate retracted (mechanical churn with zero architectural value); current state documented in `docs/reference/STATE_LAYER.md` (see note ⁷) |
```

- [ ] **Step 4: Update §7 status table P7 row**

Use the Edit tool with `replace_all: false`.

old_string:
```
| P7 | 📋 Planned | — | — | — | — |
```

new_string:
```
| P7 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p7-state-layer-design.md](./2026-04-25-p7-state-layer-design.md) | (no plan needed — see commit log) |
```

- [ ] **Step 5: Insert footnote ⁷ between footnote ⁶ and `### 4.3 Dependency Graph`**

Use the Edit tool with `replace_all: false`. The anchor is footnote ⁶'s full paragraph (one long single line) immediately followed by a blank line and then `### 4.3 Dependency Graph`.

old_string (footnote ⁶'s full paragraph + blank line + §4.3 heading — copy verbatim):
```
⁶ **P6 YAGNI retraction + doc-only investment (2026-04-25)**: P6 brainstorm audited the two roadmap-proposed deliverables — `src/session/checkpoint/` Git-style checkpoint contracts (module 7) and `src/runtime/boot.rs` 12-module assembly documentation (module 12). Findings: (a) the "Git-style checkpoint contracts" already exist in fact under different names — `SessionEventStore` trait (`src/session/store.rs`) + `SessionActor::replay()` (`src/session/actor.rs:69`) + `SessionState` pure projection (`src/session/state.rs:3`) form a complete event-sourced replay framework with test coverage (`replay_rebuilds_head_seq` at `actor.rs:230`, `replay_is_deterministic` at `state.rs:212`); adding a parallel `Checkpoint` trait beside this triple would duplicate semantics with zero present consumer; (b) actual runtime boot lives in `src/bin/aleph-server/commands/start/` totalling 6,194 LOC across 5 files (`mod.rs` 1,656 LOC, `builder/agent_init.rs` 1,905 LOC, `builder/handlers.rs` 1,951 LOC, `builder/subsystems.rs` 581 LOC, `orchestrator_init.rs` 94 LOC) — relocating it into a new `src/runtime/boot.rs` is a large refactor with no current consumer demand; (c) the stated outcome ("assembly order documented") is satisfied at lower cost by a single reference doc — `docs/reference/BOOT_ASSEMBLY.md` (~220 lines, option β: §1–§5 of TOC) — citing `file:line` rather than copy-pasting code so the doc survives modest refactors. Conclusion: `src/session/checkpoint/` retracted (parallel-abstraction zero-consumer pattern, same shape as P5's `SubagentOrchestrator`), `src/runtime/boot.rs` retracted (no consumer demand), no `src/runtime/` directory created (avoiding P0→P2 placeholder anti-pattern). Net change: 0 LOC of source code, 1 new reference doc (`BOOT_ASSEMBLY.md`). Risk unchanged 🟢 Low; estimate shortened 1 week → 1 day. See P6 design §2 for details.

### 4.3 Dependency Graph
```

new_string (same ⁶ paragraph, then blank line, then ⁷ paragraph, then blank line, then §4.3 heading):
```
⁶ **P6 YAGNI retraction + doc-only investment (2026-04-25)**: P6 brainstorm audited the two roadmap-proposed deliverables — `src/session/checkpoint/` Git-style checkpoint contracts (module 7) and `src/runtime/boot.rs` 12-module assembly documentation (module 12). Findings: (a) the "Git-style checkpoint contracts" already exist in fact under different names — `SessionEventStore` trait (`src/session/store.rs`) + `SessionActor::replay()` (`src/session/actor.rs:69`) + `SessionState` pure projection (`src/session/state.rs:3`) form a complete event-sourced replay framework with test coverage (`replay_rebuilds_head_seq` at `actor.rs:230`, `replay_is_deterministic` at `state.rs:212`); adding a parallel `Checkpoint` trait beside this triple would duplicate semantics with zero present consumer; (b) actual runtime boot lives in `src/bin/aleph-server/commands/start/` totalling 6,194 LOC across 5 files (`mod.rs` 1,656 LOC, `builder/agent_init.rs` 1,905 LOC, `builder/handlers.rs` 1,951 LOC, `builder/subsystems.rs` 581 LOC, `orchestrator_init.rs` 94 LOC) — relocating it into a new `src/runtime/boot.rs` is a large refactor with no current consumer demand; (c) the stated outcome ("assembly order documented") is satisfied at lower cost by a single reference doc — `docs/reference/BOOT_ASSEMBLY.md` (~220 lines, option β: §1–§5 of TOC) — citing `file:line` rather than copy-pasting code so the doc survives modest refactors. Conclusion: `src/session/checkpoint/` retracted (parallel-abstraction zero-consumer pattern, same shape as P5's `SubagentOrchestrator`), `src/runtime/boot.rs` retracted (no consumer demand), no `src/runtime/` directory created (avoiding P0→P2 placeholder anti-pattern). Net change: 0 LOC of source code, 1 new reference doc (`BOOT_ASSEMBLY.md`). Risk unchanged 🟢 Low; estimate shortened 1 week → 1 day. See P6 design §2 for details.

⁷ **P7 YAGNI retraction + doc-only investment (2026-04-25)**: P7 brainstorm audited the three roadmap-proposed deliverables for `src/resilience/`: (i) "decide StateDatabase home (merge into `src/session/` or new `src/state/`)", (ii) "delete gutted `src/resilience/`", (iii) "20+ consumers updated". Findings: (a) `src/resilience/` is **not gutted** — it contains 5,031 LOC of live code across 13 files (`types.rs` 682 LOC + `database/` 4,334 LOC across 12 files); the mod.rs comment "Only the database layer (StateDatabase) and shared types remain" is true but "remain ⇒ deletable" was the roadmap's false premise; (b) StateDatabase is a legitimate **single-connection multi-tenant SQLite Repository** (ApplicationRecord pattern) — 1 × `Arc<Mutex<Connection>>` shared across 11 private submodules (`events.rs`, `memory_events.rs`, `tasks.rs`, `sessions.rs`, `group_chat.rs`, `traces.rs`, `paired_users.rs`, `channel_offsets.rs`, `replay.rs`, `state_database/schema.rs`, sticker_descriptions); each submodule attaches methods via `impl StateDatabase { ... }`; only `pub mod migration` is exposed publicly; 28 consumer files all `use crate::resilience::StateDatabase` with **zero imports of `crate::resilience::database::events` or any other submodule path** — encapsulation is leak-free; (c) candidate homes both fail audit: `src/session/` already owns `SessionEventStore`/`SessionActor::replay()`/`SessionState` (P6) and absorbing 11 non-session domains' storage breaks high-cohesion; `src/state/` is a brand-new directory equivalent to a pure rename with zero architectural value; (d) splitting StateDatabase across domains (path B) would convert strong encapsulation into weak encapsulation, expand consumer import surface (`use ::EventsStore` + `use ::TasksStore` + …), and introduce SQLite multi-writer lock contention on a single DB file; (e) `types.rs` types are de-facto crate vocabulary (RiskLevel 24 consumers, Lane 10, AgentEvent 10, TaskStatus 9, SessionStatus 9) — physical migration cost vastly exceeds benefit. Conclusion: rename/relocate retracted (mechanical churn, zero architectural value, same shape as P5's `SubagentOrchestrator` and P6's `checkpoint/`); split retracted (anti-pattern); deletion retracted (false premise). Net change: 0 LOC of source code, 1 new reference doc (`docs/reference/STATE_LAYER.md`, ~120 lines, option β: §1–§5 of TOC) capturing the misleading-name + ApplicationRecord + 28-consumer + cross-domain-types facts so future engineers don't re-propose the same retracted refactor. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1 day. See P7 design §2–§3 for details.

### 4.3 Dependency Graph
```

---

## Task 3: Visual Diff Review + Commit

- [ ] **Step 1: Visual diff review of roadmap edits**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution diff docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

Expected: 4 distinct hunks (or fewer if rows are adjacent and collapse under git's default 3-line context window) corresponding to Steps 2–5 of Task 2. Read each hunk and confirm:
1. §3.3 row 8: Final-directory column ends with `(`HarnessError` + typed errors)⁷` and Action column describes the retraction with `(see note ⁷)` at the end
2. §4.2 P7 row: Risk is `🟢 Low⁷`, Estimate is `1 day⁷`, Exit Artifact mentions `STATE_LAYER.md` and `(see note ⁷)`
3. §7 P7 row: status `✅ Complete` with both dates `2026-04-25` and a link to the design doc
4. New footnote ⁷ inserted between footnote ⁶ and `### 4.3 Dependency Graph`, opening with `⁷ **P7 YAGNI retraction + doc-only investment (2026-04-25)**:` and closing with `See P7 design §2–§3 for details.`

- [ ] **Step 2: Confirm new STATE_LAYER.md is present**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```

Expected output:
```
?? docs/reference/STATE_LAYER.md
 M docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

(The leading `??` means untracked-new-file, ` M` means modified-tracked.)

- [ ] **Step 3: Stage both files and verify**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution add docs/reference/STATE_LAYER.md docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```

Expected output:
```
A  docs/reference/STATE_LAYER.md
M  docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

(Both staged: `A` for added, `M` for modified.)

- [ ] **Step 4: Commit**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution commit -m "docs: P7 — retract resilience deletion + add STATE_LAYER.md doc"
```

Expected output: `[harness-dissolution <sha>] docs: P7 — retract resilience deletion + add STATE_LAYER.md doc` with `2 files changed, ...`.

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
  - `docs/reference/STATE_LAYER.md` — new file, ~110–135 insertions
  - `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — ~5 insertions / ~3 deletions (give or take)

---

## Verification Summary

After Tasks 1–3 complete:

- ✅ `docs/reference/STATE_LAYER.md` exists with §1–§5 (option β) under ~135 lines
- ✅ Roadmap §3.3 row 8, §4.2 P7 row, §7 P7 row updated
- ✅ Footnote ⁷ added to roadmap between ⁶ and `### 4.3 Dependency Graph`
- ✅ Single commit on `harness-dissolution`, single-line message, no push, no merge to main
- ✅ No source-code changes (no cargo check / clippy required)
- ✅ Working tree clean (`git status --short` empty after the commit)

---

## Rollback

If anything goes wrong after the commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-sha>
```
This restores the roadmap to its pre-commit state (P7 row still `📋 Planned`, no footnote ⁷) and deletes `STATE_LAYER.md`. No source-code side effects.
