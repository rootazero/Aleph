# P5 — Subagent Orchestration YAGNI Retraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land one commit on the long-lived `harness-dissolution` branch that marks P5 retracted in the roadmap, with a footnote ⁵ explaining why the proposed 4-way merge and `SubagentOrchestrator` trait are not happening.

**Architecture:** Single commit, single file edited (`docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`). Five surgical Edits. No source code changes. Visual `git diff` review is the only verification gate.

**Tech Stack:** git, the Edit tool. Nothing else.

---

## Spec Reference

This plan implements `docs/superpowers/specs/2026-04-25-p5-subagents-design.md` (committed at `df183d526`). See spec §2 (Code Census Evidence) for why retraction is the right call, §3 (Key Decisions D1–D9) for the five edit sites, §4 (Commit Plan) for the verification bar.

## Operating Constraints

- **Worktree**: All work happens in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`. Do NOT touch `/Volumes/TBU4/Workspace/Aleph` (main repo).
- **Branch**: `harness-dissolution` (long-lived, P0–P7). Do NOT push. Do NOT merge to main. User decides merge timing later.
- **Commit message**: Single-line `-m` only. Multi-line HEREDOC bodies trip `block-no-verify@1.1.2` hook false-positives.
- **Verification bar**: No source-code change → no cargo check / clippy required. Visual `git diff` review is the only gate.

---

## Task 1: Roadmap Close-Out

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (5 edits + 1 commit)

- [ ] **Step 1: Update §3.3 row 11 (Subagent Orchestration)**

Use the Edit tool with `replace_all: false`:

old_string:
```
| 11 | Subagent Orchestration | **`src/subagents/`** (new home) | **Collapse 4 dirs** (agents + teams + orchestrator + group_chat) + rename `supervisor/` → `src/process_supervisor/` (not subagent) |
```

new_string:
```
| 11 | Subagent Orchestration | `src/{agents,teams,orchestrator,group_chat}/` (kept in place)⁵ | All 4 directories are healthy live code with orthogonal responsibilities; 4-way merge and `SubagentOrchestrator` trait retracted (see note ⁵). `supervisor/` → `src/process_supervisor/` rename already landed in P0. |
```

- [ ] **Step 2: Update §4.2 P5 row**

Use the Edit tool with `replace_all: false`:

old_string:
```
| **P5** | `P5-subagents` | Subagent orchestration collapse | 🔴 High | 3 weeks | `src/subagents/` from 4-way merge (agents + teams + orchestrator + group_chat); `supervisor/` renamed out; `SubagentOrchestrator` trait; Fork / Handoff / Graph modes explicit |
```

new_string:
```
| **P5** | `P5-subagents` | Subagent orchestration collapse | 🟢 Low⁵ | 1 day⁵ | 4-way merge retracted; `agents`/`teams`/`orchestrator`/`group_chat` confirmed healthy live code with orthogonal responsibilities; `SubagentOrchestrator` trait + Fork/Handoff/Graph modes retracted (see note ⁵) |
```

- [ ] **Step 3: Mark §6 open question 3 as resolved**

Use the Edit tool with `replace_all: false`:

old_string:
```
3. **P5**: Of the 5 subagent directories, `src/supervisor/` seems to be PTY subprocess supervision (unrelated to subagent orchestration). Confirm this in P5 brainstorm and relocate to `src/process_supervisor/`.
```

new_string:
```
3. **P5** ✅ **Resolved (2026-04-24, by P0)**: `src/supervisor/` was confirmed to be PTY subprocess supervision and was renamed to `src/process_supervisor/` during P0 (commit `4f96f2d66`-era work). The 4-way merge of the remaining subagent directories was subsequently retracted during P5 brainstorm — see footnote ⁵.
```

- [ ] **Step 4: Update §7 status table P5 row**

Use the Edit tool with `replace_all: false`:

old_string:
```
| P5 | 📋 Planned | — | — | — | — |
```

new_string:
```
| P5 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p5-subagents-design.md](./2026-04-25-p5-subagents-design.md) | (no plan needed — see commit log) |
```

- [ ] **Step 5: Insert footnote ⁵ after footnote ⁴**

Use the Edit tool with `replace_all: false`. The anchor is the `### 4.3 Dependency Graph` heading immediately after footnote ⁴.

old_string:
```
⁴ **P2 YAGNI retraction (2026-04-25)**: P2 brainstorm audited the three locations the roadmap proposed to merge (`src/thinker/`, `src/prompt/`, `src/harness/sections/`) plus two adjacent modules surfaced during the audit (`src/payload/`, `src/capability/`). Findings: (a) `src/prompt/` (747 lines) was dead — its sole consumer chain `payload::assembler::intent` was itself dead, and it was built on the already-removed `UnifiedIntentClassifier`; (b) `src/payload/` (~1,700 lines) and `src/capability/` (~2,500 lines) form a closed loop of test-only code — `PromptAssembler::build_prompt_with_intent_result` and `CapabilitySystem::execute()` are only called from their own unit tests; (c) `src/prompt_assembly/` (289 lines) was orphan scaffolding created in P0 awaiting P2 wiring, with zero external consumers; (d) `src/harness/sections/` was already moved during P0 (see §3.2). All four were deleted per the P1 (`compressor`)/P3 (`permission`)/P4 (`VerifyStopHook`) precedent (~5,200 LOC removed). The `PromptAssembler` / `Section` / `PromptLayer` trait commitment was retracted: `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction, and adding a parallel trait layer violates R3 (Core Minimalism) + R8 (LLM Sovereignty) + R10 (Intelligence in Prompt). The thinker→prompt_assembly directory rename (open question 2) was also retracted: a pure naming refactor touching 30 files + 200+ imports is not worth the diff cost. `src/thinker/` stays as the canonical prompt assembly home under its historical name. `src/intent/` (~150 lines, type-only) stays for gateway slash-command metadata serialization. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 2 weeks → 1 day. See P2 design §2–§3 for details.

### 4.3 Dependency Graph
```

new_string:
```
⁴ **P2 YAGNI retraction (2026-04-25)**: P2 brainstorm audited the three locations the roadmap proposed to merge (`src/thinker/`, `src/prompt/`, `src/harness/sections/`) plus two adjacent modules surfaced during the audit (`src/payload/`, `src/capability/`). Findings: (a) `src/prompt/` (747 lines) was dead — its sole consumer chain `payload::assembler::intent` was itself dead, and it was built on the already-removed `UnifiedIntentClassifier`; (b) `src/payload/` (~1,700 lines) and `src/capability/` (~2,500 lines) form a closed loop of test-only code — `PromptAssembler::build_prompt_with_intent_result` and `CapabilitySystem::execute()` are only called from their own unit tests; (c) `src/prompt_assembly/` (289 lines) was orphan scaffolding created in P0 awaiting P2 wiring, with zero external consumers; (d) `src/harness/sections/` was already moved during P0 (see §3.2). All four were deleted per the P1 (`compressor`)/P3 (`permission`)/P4 (`VerifyStopHook`) precedent (~5,200 LOC removed). The `PromptAssembler` / `Section` / `PromptLayer` trait commitment was retracted: `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction, and adding a parallel trait layer violates R3 (Core Minimalism) + R8 (LLM Sovereignty) + R10 (Intelligence in Prompt). The thinker→prompt_assembly directory rename (open question 2) was also retracted: a pure naming refactor touching 30 files + 200+ imports is not worth the diff cost. `src/thinker/` stays as the canonical prompt assembly home under its historical name. `src/intent/` (~150 lines, type-only) stays for gateway slash-command metadata serialization. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 2 weeks → 1 day. See P2 design §2–§3 for details.

⁵ **P5 YAGNI retraction (2026-04-25)**: P5 brainstorm performed a full code census of the four directories the roadmap proposed to merge into `src/subagents/`: `src/agents/` (32 files / 17,503 LOC / 27 external consumers), `src/teams/` (18 files / 9,203 LOC / 19 external consumers), `src/orchestrator/` (21 files / 4,163 LOC / 4 external consumers), `src/group_chat/` (8 files / 5,770 LOC / 4 external consumers) — totaling 79 files / 36,639 LOC / 54 external consumer files. Findings: (a) all four directories are healthy live code with active production runtime paths (gateway/execution_engine, builtin_tools, providers, thinker, A2A, Telegram); (b) responsibilities are orthogonal — `agents/` = runtime, `teams/` = lifecycle/persistence, `orchestrator/` = flow composition, `group_chat/` = channel — collapsing them into one directory would dilute responsibility boundaries rather than clarify them; (c) two earlier dead-code claims were rebutted: every `teams/` trait has a `Sqlite*` implementation (textbook trait+impl dependency-inversion), and the 9 `unimplemented!()` in `agents/swarm/context_injector.rs:671-719` are intentional `#[cfg(test)] MockTaskStore` narrow stubs; (d) the proposed `SubagentOrchestrator` trait + Fork/Handoff/Graph modes have zero current consumers — each spawn path today (`builtin_tools/sessions/spawn_tool.rs`, `builtin_tools/team/delegate.rs`, `a2a/`, `orchestrator/`) calls a concrete API directly, not through a `dyn` trait — so adding a unifying trait would violate R3 (Core Minimalism), R8 (LLM Sovereignty), and R10 (Intelligence in Prompt). Conclusion: 4-way merge retracted, no `src/subagents/` directory created (avoiding the P0→P2 placeholder anti-pattern), `SubagentOrchestrator`/Fork/Handoff/Graph trait commitment retracted. The 4 directories stay where they are. If a future 5th subagent shape eventually demands a unified entry-point, a new phase named **P8-subagent-merge** (not "P5 round 2") should revisit consolidation. Risk downgraded 🔴 High → 🟢 Low; estimate shortened 3 weeks → 1 day. See P5 design §2–§3 for details.

### 4.3 Dependency Graph
```

- [ ] **Step 6: Visual diff review**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && git diff docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

Expected: 5 distinct hunks corresponding to Steps 1–5. Read each hunk and confirm:
1. §3.3 row 11: Final Directory column now shows `src/{agents,teams,orchestrator,group_chat}/ (kept in place)⁵` and Action column describes retraction
2. §4.2 P5 row: Risk is `🟢 Low⁵`, Estimate is `1 day⁵`, Exit Artifact mentions retraction
3. §6 question 3: prefixed with `✅ Resolved (2026-04-24, by P0)` and explains both the supervisor rename (already done) and the 4-way merge retraction (linking to footnote ⁵)
4. §7 P5 row: status `✅ Complete` with both dates `2026-04-25` and a link to the design doc
5. New footnote ⁵ inserted between footnote ⁴ and `### 4.3 Dependency Graph`

- [ ] **Step 7: Stage and commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md && git status --short
```
Expected output: `M  docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (single staged modification).

Then commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution commit -m "docs(spec): mark P5 retracted; document subagent dirs healthy as-is"
```
Expected output: `[harness-dissolution <sha>] docs(spec): mark P5 retracted; document subagent dirs healthy as-is` with `1 file changed, ...`.

If the commit hook complains (`block-no-verify`, etc.), do NOT use `--no-verify`. Read the hook output, fix the underlying issue, and try again with a single-line `-m` message.

---

## Verification Summary

After Task 1 completes:

- ✅ Roadmap §3.3 row 11, §4.2 P5 row, §6 question 3, §7 P5 row updated
- ✅ Footnote ⁵ added to roadmap between ⁴ and `### 4.3 Dependency Graph`
- ✅ Single commit on `harness-dissolution`, single-line message, no push, no merge to main
- ✅ No source-code changes (no cargo check / clippy required)
- ✅ Working tree clean (`git status --short` empty after the commit)

---

## Rollback

If anything goes wrong after the commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-sha>
```
This restores the roadmap to its pre-commit state (P5 row still `📋 Planned`, no footnote ⁵).
