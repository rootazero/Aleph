# P3 Guardrails YAGNI Retraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the orphan `src/permission/` module (zero external consumers) and retract the `src/guardrails/` facade plan from the parent roadmap, in two atomic commits on `harness-dissolution`.

**Architecture:** Pure deletion + documentation. No new modules, no new traits, no physical relocation of the four live modules (`security`, `sandbox`, `approval`, `pii`). Mirrors the P1 (`compressor`) and P4 (`VerifyStopHook`) precedent.

**Tech Stack:** Rust (cargo check + cargo clippy). Markdown for roadmap.

**Worktree:** `/Volumes/TBU4/Workspace/Aleph.harness-dissolution` (branch `harness-dissolution`).

**Verification bar:** `cargo check -p alephcore` + `cargo clippy -- -D warnings` after each commit. Inherits pre-existing P0 clippy exemptions (8 warnings in `tool_output/summary.rs` + `thinker/soul.rs`); do not treat them as new regressions.

**Merge policy:** No `git push`. No `git merge` to `main`. User decides merge timing after P3 lands.

---

## Task 0: Baseline check

**Goal:** Confirm the working tree is clean and `crate::permission` truly has zero consumers before any deletion.

- [ ] **Step 1: Confirm worktree HEAD and clean status**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git status --short
git log --oneline -5
```

Expected:
- `git status --short` shows no modified or untracked files (last commit was `90c45e661` for the P3 spec)
- `git log --oneline -5` top entry is `90c45e661 docs(spec): P3 guardrails YAGNI retraction + orphan deletion design`

If status is dirty, stop and resolve before proceeding.

- [ ] **Step 2: Re-verify orphan claim**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn --include='*.rs' "crate::permission" src/ | grep -v "^src/permission/"
```

Expected: zero output (no matches outside `src/permission/` itself).

If any match appears, **stop**: the orphan claim is false, escalate to controller before proceeding with Task 1.

- [ ] **Step 3: Capture baseline cargo state**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -10
```

Expected: `Finished \`dev\` profile` — green. Ignore the 7 pre-existing warnings in `alephcore` (they originate from `tool_output/summary.rs` and `thinker/soul.rs`, inherited from P0 baseline `bba189278`).

---

## Task 1: Delete orphan `src/permission/` module

**Files:**
- Delete: `src/permission/mod.rs`
- Delete: `src/permission/config.rs`
- Delete: `src/permission/error.rs`
- Delete: `src/permission/manager.rs`
- Delete: `src/permission/rule.rs`
- Delete: `src/permission/` (directory itself, after files removed)
- Modify: `src/lib.rs` — remove line 72 `pub mod permission;`

- [ ] **Step 1: Delete the entire directory via git**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git rm -r src/permission/
```

Expected output (paths may vary in order):
```
rm 'src/permission/config.rs'
rm 'src/permission/error.rs'
rm 'src/permission/manager.rs'
rm 'src/permission/mod.rs'
rm 'src/permission/rule.rs'
```

Verify with:
```bash
ls src/permission 2>&1
```
Expected: `ls: src/permission: No such file or directory`.

- [ ] **Step 2: Remove the module declaration from `src/lib.rs`**

Use Edit tool (not sed) to change `src/lib.rs`:

`old_string`:
```
pub mod payload;
pub mod permission;
pub mod pii;
```

`new_string`:
```
pub mod payload;
pub mod pii;
```

Verify with:
```bash
grep -n "pub mod permission" src/lib.rs
```
Expected: zero output.

- [ ] **Step 3: Sanity-check the wider tree for any remaining references**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn --include='*.rs' "crate::permission" src/
```
Expected: zero output.

Run:
```bash
grep -rn --include='*.rs' "use crate::permission" src/
```
Expected: zero output.

If either grep returns matches, **stop**: there is a hidden consumer that must be addressed before commit.

- [ ] **Step 4: Run `cargo check`**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore
```

Expected: `Finished \`dev\` profile`. Same 7 pre-existing warnings as baseline; no new errors. If new errors appear (especially `E0432 unresolved import` or `E0433 cannot find module`), **stop** and report — there is an unaccounted consumer.

- [ ] **Step 5: Run `cargo clippy -- -D warnings`**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20
```

Expected: same pre-existing P0 clippy exemptions (8 errors in `tool_output/summary.rs` `clippy::dead_code` + `thinker/soul.rs` `clippy::collapsible_else_if`). Do not treat as new regressions. **Important**: deletion must not introduce new warnings beyond this baseline.

If a new warning appears that was not present at baseline, fix it before commit.

- [ ] **Step 6: Stage and commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add src/lib.rs
git status --short
```

Expected `git status --short` output includes:
- `M  src/lib.rs`
- `D  src/permission/config.rs`
- `D  src/permission/error.rs`
- `D  src/permission/manager.rs`
- `D  src/permission/mod.rs`
- `D  src/permission/rule.rs`

Then commit:
```bash
git commit -m "permission: delete orphan crate::permission module (0 consumers)"
```

Expected:
- Commit succeeds (no pre-commit hook rejection — this is a short single-line message)
- `1 file changed, 1 deletion(-)` for `src/lib.rs` plus 5 file deletions

- [ ] **Step 7: Post-commit sanity check**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git log --oneline -3
git show --stat HEAD | head -15
cargo check -p alephcore 2>&1 | tail -5
```

Expected:
- HEAD is the new commit
- `git show --stat HEAD` shows 6 files changed (1 modify + 5 delete)
- `cargo check` still green

---

## Task 2: Roadmap close-out

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (4 edits across §3.3, §4.2, §4.2 footnotes, §7)

- [ ] **Step 1: Edit §3.3 module 9 row (trim trait list, add ³)**

Use Edit tool on `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`:

`old_string`:
```
| 9 | Guardrails | **`src/guardrails/`** (new facade) | Aggregate security/sandbox/permission/approval/pii; InputGuard/OutputGuard/ToolCallGuard |
```

`new_string`:
```
| 9 | Guardrails | `src/{security,sandbox,approval,pii}/` (kept in place)³ | Orphan `src/permission/` deleted; facade and InputGuard/OutputGuard/ToolCallGuard traits retracted (see note ³) |
```

- [ ] **Step 2: Edit §4.2 P3 row**

`old_string`:
```
| **P3** | `P3-guardrails` | Guardrails facade | 🟡 Medium | 1.5 weeks | `src/guardrails/` with InputGuard/OutputGuard/ToolCallGuard; delegates to existing backing stores |
```

`new_string`:
```
| **P3** | `P3-guardrails` | Guardrails facade | 🟢 Low³ | 1–2 hours³ | Orphan `src/permission/` deleted; facade plan retracted (see note ³) |
```

- [ ] **Step 3: Add footnote ³ after the existing footnote ²**

`old_string`:
```
² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4 brainstorm, the roadmap's "rule / visual / LLM-judge contracts" commitment was retracted. Aleph's verification logic lives entirely in prompt templates (see `src/thinker/layers/agent_role.rs` VERDICT block) per R8/R10; no Rust-level verifier trait has a present consumer. A separate finding: `VerifyStopHook` (194 lines in `src/verification/verify_stop_hook.rs`) was orphaned code — zero production instantiations since its April 2026 introduction in commit b54877d7f — and was deleted per the P1 compressor precedent (dead code with zero consumers gets removed, not renamed). Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P4 design §2–§4 for details.

### 4.3 Dependency Graph
```

`new_string`:
```
² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4 brainstorm, the roadmap's "rule / visual / LLM-judge contracts" commitment was retracted. Aleph's verification logic lives entirely in prompt templates (see `src/thinker/layers/agent_role.rs` VERDICT block) per R8/R10; no Rust-level verifier trait has a present consumer. A separate finding: `VerifyStopHook` (194 lines in `src/verification/verify_stop_hook.rs`) was orphaned code — zero production instantiations since its April 2026 introduction in commit b54877d7f — and was deleted per the P1 compressor precedent (dead code with zero consumers gets removed, not renamed). Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P4 design §2–§4 for details.

³ **P3 YAGNI retraction + orphan deletion (2026-04-25)**: P3 brainstorm audited the five modules originally proposed for the guardrails facade. Findings: (a) `src/permission/` was orphan code — zero external consumers since its April 2026 introduction in commit `1f7b33931` — and was deleted per the P1 (`compressor`) / P4 (`VerifyStopHook`) precedent (dead code with zero consumers gets removed, not relocated); (b) the four live modules (`security`, `sandbox`, `approval`, `pii`) serve genuinely distinct domains with distinct consumer footprints, so a parent `src/guardrails/` directory was rejected as adding hierarchy without solving any pain; (c) the planned `InputGuard` / `OutputGuard` / `ToolCallGuard` traits had no present consumer and were retracted (R3 + YAGNI). A separate fragmentation finding — three parallel exec-approval implementations (`src/exec/approval/`, `src/sandbox/exec_approval/`, `src/tools/middleware/permission/`) and six distinct `ApprovalDecision` types across the codebase — is layered/domain-distinct rather than a name collision, and is deferred to a future phase. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P3 design §2–§3 for details.

### 4.3 Dependency Graph
```

- [ ] **Step 4: Edit §7 P3 status row**

`old_string`:
```
| P3 | 📋 Planned | — | — | — | — |
```

`new_string`:
```
| P3 | ✅ Complete | 2026-04-25 | 2026-04-25 | [2026-04-25-p3-guardrails-design.md](./2026-04-25-p3-guardrails-design.md) | [2026-04-25-p3-guardrails.md](../plans/2026-04-25-p3-guardrails.md) |
```

- [ ] **Step 5: Visual diff review**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git diff docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md | head -80
```

Expected:
- 4 hunks modified — §3.3 row 9, §4.2 P3 row, footnote ³ added after footnote ², §7 P3 row
- No accidental changes to other rows or footnotes
- ³ symbol appears consistently (matching ¹ and ²)

- [ ] **Step 6: Stage and commit**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git commit -m "docs(spec): mark P3 complete; record YAGNI retraction and orphan deletion"
```

Note: keep the message single-line. The pre-commit hook `block-no-verify@1.1.2` has been observed to false-positive on multi-line HEREDOC bodies (P4 task 2 hit this). Detailed rationale lives in footnote ³ inside the file itself.

Expected:
- Commit succeeds (single-line message bypasses the hook regex false-positive)
- `1 file changed` with ~6 insertions / ~3 deletions

- [ ] **Step 7: Final verification**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git log --oneline -5
git status --short
```

Expected:
- Top two commits are the two P3 commits
- `git status --short` shows clean working tree
- No `git push` — branch stays local on `harness-dissolution`

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -3
```

Expected: still green (this commit was docs-only, but a final sanity check costs nothing).

---

## Closing Checklist

- [ ] `src/permission/` directory does not exist
- [ ] `src/lib.rs` no longer declares `pub mod permission;`
- [ ] `cargo check -p alephcore` green
- [ ] `cargo clippy -p alephcore -- -D warnings` green (P0 exemptions inherited; no new warnings)
- [ ] `grep -rn 'crate::permission' src/` returns zero matches
- [ ] Roadmap §3.3 row 9 trimmed and references footnote ³
- [ ] Roadmap §4.2 P3 row shows 🟢 Low³ + 1–2 hours³
- [ ] Roadmap footnote ³ present and complete
- [ ] Roadmap §7 P3 row shows ✅ Complete with 2026-04-25 dates and links
- [ ] Two commits on `harness-dissolution`, no push, no merge to `main`
