# P2 — Prompt Assembly YAGNI Retraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete four dead-code modules (`src/prompt/`, `src/payload/`, `src/capability/`, `src/prompt_assembly/`) totaling ~5,200 LOC, then close out P2 in the harness-dissolution roadmap with a YAGNI-retraction footnote.

**Architecture:** Two atomic commits on the long-lived `harness-dissolution` branch. Commit 1 deletes 4 directories + 4 lines from `src/lib.rs` + 1 line from `src/search/provider.rs`. Commit 2 updates the roadmap doc (§3.3 row 5, §4.2 P2 row, §6 open question 2, §7 status table, new footnote ⁴). No source-code logic changes; cargo check/clippy stay green by virtue of the modules forming a closed dead-code loop.

**Tech Stack:** Rust (alephcore crate), cargo, git. No new dependencies.

---

## Spec Reference

This plan implements `docs/superpowers/specs/2026-04-25-p2-prompt-assembly-design.md`. See spec §2 (Code Census) for why each module is dead, §3 (Key Decisions) for D1–D10, §4 (Commit Plan) for verification bar.

## Operating Constraints

- **Worktree**: All work happens in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`. Do NOT touch `/Volumes/TBU4/Workspace/Aleph` (main repo).
- **Branch**: `harness-dissolution` (long-lived, P0–P7). Do NOT push. Do NOT merge to main. User decides merge timing later.
- **Commit message**: Single-line `-m` only. Multi-line HEREDOC bodies trip `block-no-verify@1.1.2` hook false-positives.
- **Verification bar** (inherit from P0): `cargo check -p alephcore` green + `cargo clippy -- -D warnings` green after each commit. P0 already accepted 7 pre-existing alephcore lib warnings + 8 pre-existing clippy errors in `tool_output/summary.rs` and `thinker/soul.rs`; those exemptions stay. New warnings introduced by P2 work are NOT acceptable.

---

## Task 0: Baseline Check

**Files:** None (read-only verification).

- [ ] **Step 1: Confirm worktree and branch**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && pwd && git branch --show-current
```
Expected output:
```
/Volumes/TBU4/Workspace/Aleph.harness-dissolution
harness-dissolution
```

- [ ] **Step 2: Confirm clean working tree**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```
Expected output: empty (no changes).

- [ ] **Step 3: Confirm zero live consumers of the four target modules**

Run:
```bash
grep -rn "use crate::prompt::\|use crate::prompt;\|use crate::payload::\|use crate::payload;\|use crate::capability::\|use crate::capability;\|use crate::prompt_assembly::\|use crate::prompt_assembly;" /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src --include="*.rs" | grep -v "src/prompt/" | grep -v "src/payload/" | grep -v "src/capability/" | grep -v "src/prompt_assembly/"
```
Expected output: empty (no external consumers — the four modules import each other but nothing else imports them).

- [ ] **Step 4: Confirm baseline cargo check is green**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && cargo check -p alephcore 2>&1 | tail -5
```
Expected output: `Finished` line (warnings allowed; the 7 pre-existing alephcore lib warnings inherited from P0 are fine).

---

## Task 1: Delete Four Dead Modules

**Files:**
- Delete: `src/prompt/` (entire directory: `builder.rs`, `conversational.rs`, `executor.rs`, `mod.rs`, `templates.rs`)
- Delete: `src/payload/` (entire directory: `builder.rs`, `capability.rs`, `context_format.rs`, `intent.rs`, `mod.rs`, plus `assembler/{capability.rs, context.rs, core.rs, formatters.rs, intent.rs, mod.rs, tools.rs}`)
- Delete: `src/capability/` (entire directory: `declaration.rs`, `mod.rs`, `request.rs`, `response_parser.rs`, `strategy.rs`, `system.rs`, plus `strategies/{mcp.rs, memory.rs, mod.rs, skills.rs}`)
- Delete: `src/prompt_assembly/` (entire directory: `mod.rs`, plus `sections/{mod.rs, browser.md, code_exec.md, output_style.md, persistence.md, risk_actions.md, subagent.md, task_philosophy.md, tool_grammar.md}`)
- Modify: `src/lib.rs` (remove 4 `pub mod` lines)
- Modify: `src/search/provider.rs` (remove 1 doc-comment line)

- [ ] **Step 1: Delete the four directories**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && rm -rf src/prompt src/payload src/capability src/prompt_assembly
```

- [ ] **Step 2: Verify directories are gone**

Run:
```bash
ls /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src/prompt /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src/payload /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src/capability /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src/prompt_assembly 2>&1
```
Expected output: 4 lines of `ls: ...: No such file or directory` — this is the desired post-deletion state.

- [ ] **Step 3: Remove `pub mod capability;` from `src/lib.rs:43`**

Use the Edit tool with `replace_all: false`:

old_string:
```
pub mod bundled;
pub mod capability;
pub mod clarification;
```

new_string:
```
pub mod bundled;
pub mod clarification;
```

- [ ] **Step 4: Remove `pub mod payload;` from `src/lib.rs` (was line 71)**

Use the Edit tool with `replace_all: false`:

old_string:
```
pub mod orchestrator;
pub mod payload;
pub mod pii;
```

new_string:
```
pub mod orchestrator;
pub mod pii;
```

- [ ] **Step 5: Remove `pub mod prompt;` and `pub mod prompt_assembly;` from `src/lib.rs` (were lines 73–74)**

Use the Edit tool with `replace_all: false`:

old_string:
```
pub mod pii;
pub mod prompt;
pub mod prompt_assembly;
pub mod providers;
```

new_string:
```
pub mod pii;
pub mod providers;
```

- [ ] **Step 6: Delete the dangling `CapabilityExecutor` reference from `src/search/provider.rs:11`**

Use the Edit tool with `replace_all: false`:

old_string:
```
/// Unified interface for search providers
///
/// All search backends (Tavily, Google, SearXNG, etc.) implement this trait
/// to provide a consistent API to the CapabilityExecutor.
#[async_trait]
```

new_string:
```
/// Unified interface for search providers
///
/// All search backends (Tavily, Google, SearXNG, etc.) implement this trait.
#[async_trait]
```

- [ ] **Step 7: Run cargo check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && cargo check -p alephcore 2>&1 | tail -10
```
Expected output: `Finished` line. Pre-existing alephcore warnings (≤7) are fine. NO new errors. If `cargo check` fails with `unresolved import` or similar, do not proceed — investigate the failure (most likely a hidden consumer the brainstorm missed; spec §6 risk 1 says revert and report).

- [ ] **Step 8: Run cargo clippy**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -10
```
Expected output: `Finished` (with the 8 pre-existing P0 clippy errors in `tool_output/summary.rs` and `thinker/soul.rs` — those are inherited exemptions and are tolerated by P0/P1/P3/P4 precedent). No NEW clippy errors introduced by P2 work.

If clippy reports new errors that didn't exist before P2 (i.e., not in the P0-inherited list), they were caused by the deletion (e.g., dead code that was reachable only via the deleted modules). Investigate and either delete that dead code in this same commit or revert and ask the user.

- [ ] **Step 9: Confirm no dangling references**

Run:
```bash
grep -rn "crate::prompt::\|crate::payload::\|crate::capability::\|crate::prompt_assembly::" /Volumes/TBU4/Workspace/Aleph.harness-dissolution/src --include="*.rs"
```
Expected output: empty (or only matches inside `src/extension/capability::*` paths, which are unrelated — those are paths into `src/extension/capability.rs`, the live enum, NOT the deleted `src/capability/` module).

- [ ] **Step 10: Stage and commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && git add -A src/ && git status --short
```
Expected output: D-prefixed lines for all deleted files in the four directories, plus M lines for `src/lib.rs` and `src/search/provider.rs`. Should be roughly 38 deletions + 2 modifications.

Then commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution commit -m "cleanup: delete dead prompt/payload/capability/prompt_assembly modules (~5,200 LOC)"
```
Expected output: `[harness-dissolution <sha>] cleanup: delete dead prompt/payload/capability/prompt_assembly modules (~5,200 LOC)` followed by file change summary.

If the commit hook complains (`block-no-verify`, etc.), do NOT use `--no-verify`. Read the hook output, fix the underlying issue, and try again with a single-line `-m` message.

---

## Task 2: Roadmap Close-Out

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (4 small edits + 1 footnote insertion)

- [ ] **Step 1: Update §3.3 row 5 (Prompt Assembly)**

Use the Edit tool with `replace_all: false`:

old_string:
```
| 5 | Prompt Assembly | **`src/prompt_assembly/`** | **Merge 3 locations** (thinker + prompt + harness/sections) |
```

new_string:
```
| 5 | Prompt Assembly | `src/thinker/` (kept in place)⁴ | Dead code in `prompt`/`payload`/`capability`/`prompt_assembly` deleted; 3-way merge and `PromptAssembler` trait retracted (see note ⁴) |
```

- [ ] **Step 2: Update §4.2 P2 row**

Use the Edit tool with `replace_all: false`:

old_string:
```
| **P2** | `P2-prompt-assembly` | Prompt assembly consolidation | 🟡 Medium | 2 weeks | 3-way merge → `src/prompt_assembly/`; `PromptAssembler` trait |
```

new_string:
```
| **P2** | `P2-prompt-assembly` | Prompt assembly consolidation | 🟢 Low⁴ | 1 day⁴ | `prompt`/`payload`/`capability`/`prompt_assembly` deleted (~5,200 LOC); facade trait plan retracted (see note ⁴) |
```

- [ ] **Step 3: Mark §6 open question 2 as resolved**

Use the Edit tool with `replace_all: false`:

old_string:
```
2. **P2**: Should the unified prompt directory be named `src/prompt_assembly/` or keep `src/thinker/` (historical name)? `thinker` is misleading under R8 (thinking belongs to the LLM, not the assembler), so `prompt_assembly` is preferred, but it's a rename of ~30 files.
```

new_string:
```
2. **P2** ✅ **Resolved (2026-04-25)**: P2 brainstorm chose to keep `src/thinker/` under its historical name. The `thinker` → `prompt_assembly` rename was retracted as a pure naming refactor touching 30 files + 200+ import sites with no behavioral payoff. The `src/prompt_assembly/` scaffold (created in P0 awaiting P2 wiring) was deleted as orphan code per the P1/P3/P4 retraction precedent. See P2 design §1 (Anti-goals) and footnote ⁴.
```

- [ ] **Step 4: Update §7 status table P2 row**

Use the Edit tool with `replace_all: false`:

old_string:
```
| P2 | 📋 Planned | — | — | — | — |
```

new_string:
```
| P2 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p2-prompt-assembly-design.md](./2026-04-25-p2-prompt-assembly-design.md) | (no plan needed — see commit log) |
```

- [ ] **Step 5: Insert footnote ⁴ after footnote ³**

Use the Edit tool with `replace_all: false`:

old_string:
```
³ **P3 YAGNI retraction + orphan deletion (2026-04-25)**: P3 brainstorm audited the five modules originally proposed for the guardrails facade. Findings: (a) `src/permission/` was orphan code — zero external consumers since its April 2026 introduction in commit `1f7b33931` — and was deleted per the P1 (`compressor`) / P4 (`VerifyStopHook`) precedent (dead code with zero consumers gets removed, not relocated); (b) the four live modules (`security`, `sandbox`, `approval`, `pii`) serve genuinely distinct domains with distinct consumer footprints, so a parent `src/guardrails/` directory was rejected as adding hierarchy without solving any pain; (c) the planned `InputGuard` / `OutputGuard` / `ToolCallGuard` traits had no present consumer and were retracted (R3 + YAGNI). A separate fragmentation finding — three parallel exec-approval implementations (`src/exec/approval/`, `src/sandbox/exec_approval/`, `src/tools/middleware/permission/`) and six distinct `ApprovalDecision` types across the codebase — is layered/domain-distinct rather than a name collision, and is deferred to a future phase. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P3 design §2–§3 for details.

### 4.3 Dependency Graph
```

new_string:
```
³ **P3 YAGNI retraction + orphan deletion (2026-04-25)**: P3 brainstorm audited the five modules originally proposed for the guardrails facade. Findings: (a) `src/permission/` was orphan code — zero external consumers since its April 2026 introduction in commit `1f7b33931` — and was deleted per the P1 (`compressor`) / P4 (`VerifyStopHook`) precedent (dead code with zero consumers gets removed, not relocated); (b) the four live modules (`security`, `sandbox`, `approval`, `pii`) serve genuinely distinct domains with distinct consumer footprints, so a parent `src/guardrails/` directory was rejected as adding hierarchy without solving any pain; (c) the planned `InputGuard` / `OutputGuard` / `ToolCallGuard` traits had no present consumer and were retracted (R3 + YAGNI). A separate fragmentation finding — three parallel exec-approval implementations (`src/exec/approval/`, `src/sandbox/exec_approval/`, `src/tools/middleware/permission/`) and six distinct `ApprovalDecision` types across the codebase — is layered/domain-distinct rather than a name collision, and is deferred to a future phase. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P3 design §2–§3 for details.

⁴ **P2 YAGNI retraction (2026-04-25)**: P2 brainstorm audited the three locations the roadmap proposed to merge (`src/thinker/`, `src/prompt/`, `src/harness/sections/`) plus two adjacent modules surfaced during the audit (`src/payload/`, `src/capability/`). Findings: (a) `src/prompt/` (747 lines) was dead — its sole consumer chain `payload::assembler::intent` was itself dead, and it was built on the already-removed `UnifiedIntentClassifier`; (b) `src/payload/` (~1,700 lines) and `src/capability/` (~2,500 lines) form a closed loop of test-only code — `PromptAssembler::build_prompt_with_intent_result` and `CapabilitySystem::execute()` are only called from their own unit tests; (c) `src/prompt_assembly/` (289 lines) was orphan scaffolding created in P0 awaiting P2 wiring, with zero external consumers; (d) `src/harness/sections/` was already moved during P0 (see §3.2). All four were deleted per the P1 (`compressor`)/P3 (`permission`)/P4 (`VerifyStopHook`) precedent (~5,200 LOC removed). The `PromptAssembler` / `Section` / `PromptLayer` trait commitment was retracted: `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction, and adding a parallel trait layer violates R3 (Core Minimalism) + R8 (LLM Sovereignty) + R10 (Intelligence in Prompt). The thinker→prompt_assembly directory rename (open question 2) was also retracted: a pure naming refactor touching 30 files + 200+ imports is not worth the diff cost. `src/thinker/` stays as the canonical prompt assembly home under its historical name. `src/intent/` (~150 lines, type-only) stays for gateway slash-command metadata serialization. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 2 weeks → 1 day. See P2 design §2–§3 for details.

### 4.3 Dependency Graph
```

- [ ] **Step 6: Visual diff review**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && git diff docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```
Expected: 5 distinct hunks corresponding to Steps 1–5. Read each hunk and confirm:
1. §3.3 row 5: "Final Directory" cell now shows `src/thinker/ (kept in place)⁴` and "Action" cell describes deletion + retraction
2. §4.2 P2 row: Risk is `🟢 Low⁴`, Estimate is `1 day⁴`, Exit Artifact mentions deletion
3. §6 question 2: prefixed with `✅ Resolved (2026-04-25)` and explains rename retraction
4. §7 P2 row: status `✅ Complete` with both dates `2026-04-25` and a link to the design doc
5. New footnote ⁴ inserted between footnote ³ and `### 4.3 Dependency Graph`

- [ ] **Step 7: Stage and commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md && git status --short
```
Expected output: `M  docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (single staged modification).

Then commit:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution commit -m "docs(spec): mark P2 complete; record YAGNI retraction in roadmap"
```
Expected output: `[harness-dissolution <sha>] docs(spec): mark P2 complete; record YAGNI retraction in roadmap` with `1 file changed, ...`.

---

## Task 3: Final Verification

**Files:** None (read-only verification).

- [ ] **Step 1: Confirm both commits landed on `harness-dissolution`**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution log --oneline -3
```
Expected output: 3 lines, with the top two being:
```
<sha2> docs(spec): mark P2 complete; record YAGNI retraction in roadmap
<sha1> cleanup: delete dead prompt/payload/capability/prompt_assembly modules (~5,200 LOC)
```
The third line should be the P2 spec commit `4c4fc0503 docs(spec): P2 prompt-assembly YAGNI retraction + dead-module deletion design` (or similar — written before this plan started executing).

- [ ] **Step 2: Confirm working tree is clean**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --short
```
Expected output: empty.

- [ ] **Step 3: Confirm no main-branch leakage**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution log main..harness-dissolution --oneline
```
Expected output: list of commits ahead of main on `harness-dissolution`. Should INCLUDE the two P2 commits from Task 1 + Task 2 plus the P2 spec commit, plus all P1/P3/P4 commits not yet merged. Should NOT be empty (means everything stayed on the branch).

- [ ] **Step 4: Confirm cargo check + clippy still green at the new HEAD**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution && cargo check -p alephcore 2>&1 | tail -3 && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -3
```
Expected: both end in `Finished ...`. Pre-existing P0 alephcore warnings + P0 clippy exemptions are tolerated; no NEW issues.

---

## Verification Summary

After all tasks complete:

- ✅ 4 dead directories deleted (`src/prompt/`, `src/payload/`, `src/capability/`, `src/prompt_assembly/`)
- ✅ 4 `pub mod` declarations removed from `src/lib.rs`
- ✅ 1 doc-comment line removed from `src/search/provider.rs`
- ✅ Roadmap §3.3 row 5, §4.2 P2 row, §6 question 2, §7 P2 row updated
- ✅ Footnote ⁴ added to roadmap
- ✅ `cargo check -p alephcore` green
- ✅ `cargo clippy -p alephcore --lib -- -D warnings` green (P0 exemptions inherited)
- ✅ Two commits on `harness-dissolution`, no push, no merge to main
- ✅ Approximately 38 file deletions + 7 line modifications across 2 commits, totaling ~5,200 LOC removed

---

## Rollback

If anything goes wrong after Commit 1:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-1-sha>
```
This restores all four deleted directories + the lib.rs lines + the search/provider.rs comment line exactly.

If anything goes wrong after Commit 2:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution revert <commit-2-sha>
```
This restores the roadmap to its pre-Commit-2 state (P2 row still `📋 Planned`, no footnote ⁴).
