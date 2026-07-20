# P4 Verification Module Minimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete orphaned `src/verification/verify_stop_hook.rs` (194 lines, zero production consumers); rewrite `src/verification/mod.rs` doc to document Aleph's prompt-driven verification model; update roadmap with P4 completion + YAGNI record. No new traits introduced.

**Architecture:** Pure code deletion + doc edits, same minimal-scope style as P1's src/compressor/ removal. Rust-level `Verifier` / `LlmJudge` / `VisualDiffer` traits explicitly rejected per R8/R10; verification logic remains entirely in the `thinker/layers/agent_role.rs` VERDICT prompt template.

**Tech Stack:** Rust / cargo workspace (`alephcore`). Verification via `cargo check`, `cargo clippy -- -D warnings`, `just test-all`. No HTTP smoke test needed (deleted code was never in the runtime path).

**Worktree:** All work in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution` on branch `harness-dissolution`. Do NOT operate in `/Volumes/TBU4/Workspace/Aleph` (main repo).

---

## File Structure — Changes Map

### Deleted

- `src/verification/verify_stop_hook.rs` (194 lines, including ~11 embedded unit tests)

### Modified

- `src/verification/mod.rs` — remove `pub mod verify_stop_hook;` declaration; rewrite module doc comment
- `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — flip P4 status row in §7, update §4.2 row + add footnote ²

### Unchanged

- `src/verification/stop_hooks.rs` — live infrastructure (StopHookHandler trait, ShellStopHook, execute_stop_hooks); consumers at `harness/agent.rs`, `harness/deps.rs`, `orchestrator/harness_bridge.rs`, `harness/tests/task10_wiring.rs`
- `src/thinker/layers/agent_role.rs` — holds the VERDICT prompt template (Aleph's actual verification engine)
- All consumer files (nothing imports `VerifyStopHook` externally, so no imports to update)

---

## Task 0: Pre-flight Baseline

**Files:** None modified; read-only checks.

- [ ] **Step 1: Confirm worktree branch and clean state**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution branch --show-current
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --porcelain
```

Expected: branch `harness-dissolution`; working tree clean. If dirty with unrelated work, STOP and escalate.

- [ ] **Step 2: Baseline `cargo check`**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -5
```

Expected: PASS. Record the current HEAD SHA (should be `ea2c6827d` — the P4 spec commit).

- [ ] **Step 3: Confirm VerifyStopHook is truly orphaned**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn "VerifyStopHook" --include="*.rs" src/ | grep -v "^src/verification/verify_stop_hook.rs"
```

Expected: NO output. If any line appears, STOP — the spec assumption is wrong and P4 needs to be re-scoped. Escalate.

- [ ] **Step 4: Snapshot baseline file count and test count**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
ls src/verification/*.rs | wc -l                        # expect 3 (mod.rs + stop_hooks.rs + verify_stop_hook.rs)
wc -l src/verification/verify_stop_hook.rs              # expect ≈ 194
```

Record these numbers as the baseline for final reconciliation.

---

## Task 1: Delete VerifyStopHook + Rewrite mod.rs (Commit 1)

**Files:**
- Delete: `src/verification/verify_stop_hook.rs`
- Modify: `src/verification/mod.rs` (whole file rewritten — ~8 lines → ~20 lines)

- [ ] **Step 1: Delete `src/verification/verify_stop_hook.rs` via git**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git rm src/verification/verify_stop_hook.rs
```

Expected: one-line deletion message from git. `git status --short` should show `D  src/verification/verify_stop_hook.rs`.

- [ ] **Step 2: Overwrite `src/verification/mod.rs` with the new content**

Use the Write tool (or equivalent) to replace the full contents of `src/verification/mod.rs` with:

```rust
//! Verification — stop-hook infrastructure for Aleph's prompt-driven
//! verification model.
//!
//! Aleph's verification logic lives entirely in prompts (see
//! `src/thinker/layers/agent_role.rs`): agents are instructed to emit a
//! `VERDICT: PASS|FAIL|PARTIAL` block summarizing their self-checks
//! before stopping. Per redlines R8 (LLM Sovereignty) and R10
//! (Intelligence Lives in the Prompt), no Rust-level verifier, judge,
//! or critic is introduced. The `StopHookHandler` trait below hosts
//! the generic stop-interception mechanism plus `ShellStopHook` for
//! shell-command hooks.
//!
//! A separate `VerifyStopHook` Rust struct existed from April 2026
//! through P0 but was never wired into production (zero instantiation
//! sites outside its own tests). It was deleted in P4 (2026-04-24)
//! because the prompt-level mechanism fully covers the use case.
//! Retrievable from git history at commit b54877d7f if future work
//! requires a Rust-layer verdict enforcer.

pub mod stop_hooks;
```

Verify the file now contains exactly that content — no leftover `pub mod verify_stop_hook;` line, no previous doc comments.

- [ ] **Step 3: Verify grep cleanliness**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn "VerifyStopHook\|verify_stop_hook" --include="*.rs" src/
```

Expected: NO output at all — not as import, not as file path, not as module declaration. If any match remains, identify and fix before proceeding (e.g., a test file that still imports the struct, though the spec says none should exist).

- [ ] **Step 4: Verify `cargo check` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -10
```

Expected: PASS. If any error mentions `unresolved import` or `cannot find VerifyStopHook`, the grep in Step 3 missed a consumer — find and fix, then re-run.

- [ ] **Step 5: Verify `cargo clippy` passes (same level as baseline)**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -10
```

Expected: same level as Task 0 baseline — the 8 pre-existing P0-inherited errors (dead code in `tool_output/summary.rs`, `obfuscated_if_else` in `thinker/soul.rs`) remain present. **No new errors introduced.** If any new error appears, investigate — it's unexpected given that P4 only removes code.

- [ ] **Step 6: Run `just test-all` to confirm no test regressions**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
just test-all 2>&1 | tail -20
```

Expected: all tests green. Test count will drop by ~11 relative to the pre-P4 baseline (the embedded unit tests inside the deleted file). The pre-existing `check-phase5-exit.sh` false positive (documented in P1) will trip again — inherit its exemption; it is not a P4 regression.

- [ ] **Step 7: Verify final state**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
ls src/verification/*.rs              # expect exactly: mod.rs  stop_hooks.rs
git status --short                    # expect exactly: D verify_stop_hook.rs + M mod.rs
```

- [ ] **Step 8: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add -A src/verification/
git commit -m "$(cat <<'EOF'
verification: delete orphan VerifyStopHook (0 consumers)

VerifyStopHook was introduced in commit b54877d7f (2026-04-01)
alongside the VERDICT prompt template in thinker/layers/agent_role.rs.
Only the prompt half was ever wired into production; the Rust struct
had zero instantiation sites outside its own tests across its entire
lifetime. Per the P1 compressor precedent (dead code with zero
consumers gets deleted), the struct and its 194-line file are
removed here. Git preserves the implementation at b54877d7f if
future work needs a Rust-layer verdict enforcer.

src/verification/mod.rs rewritten to document Aleph's prompt-driven
verification model (R8/R10) and the deletion provenance. The generic
StopHookHandler trait + ShellStopHook in stop_hooks.rs are untouched
— they are live infrastructure consumed by harness/deps,
harness/agent, orchestrator/harness_bridge, and the task10 wiring
tests.

Commit 1 of 2 for P4 verification module minimization.
EOF
)"
```

Confirm `git status` shows clean tree and `git log --oneline -1` shows the new commit.

---

## Task 2: Roadmap Update (Commit 2)

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` (three targeted edits: §4.2 row + footnote, §7 status row)

- [ ] **Step 1: Update the §4.2 P4 phase row**

Find in `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` the line (around line 151):
```markdown
| **P4** | `P4-verification` | Verification & feedback loop | 🟡 Medium | 1.5 weeks | `src/verification/` absorbs stop_hooks; rule / visual / LLM-judge contracts |
```

Replace with:
```markdown
| **P4** | `P4-verification` | Verification & feedback loop | 🟢 Low² | 1–2 hours² | `src/verification/` houses StopHookHandler + ShellStopHook only (see note ²) |
```

- [ ] **Step 2: Add the new footnote ² after the existing ¹**

Locate the footnote ¹ block (added in P1, immediately after the `**Total**: ~13.5 weeks / ~3.5 months.` line around line 156–158). Append footnote ² on a new paragraph below it:

```markdown
² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4 brainstorm, the roadmap's "rule / visual / LLM-judge contracts" commitment was retracted. Aleph's verification logic lives entirely in prompt templates (see `src/thinker/layers/agent_role.rs` VERDICT block) per R8/R10; no Rust-level verifier trait has a present consumer. A separate finding: `VerifyStopHook` (194 lines in `src/verification/verify_stop_hook.rs`) was orphaned code — zero production instantiations since its April 2026 introduction in commit b54877d7f — and was deleted per the P1 compressor precedent (dead code with zero consumers gets removed, not renamed). Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 1.5 weeks → 1–2 hours. See P4 design §2–§4 for details.
```

- [ ] **Step 3: Update the §7 status table P4 row**

Find (around line 225, the row below P3):
```markdown
| P4 | 📋 Planned | — | — | — | — |
```

Replace with:
```markdown
| P4 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p4-verification-design.md](./2026-04-24-p4-verification-design.md) | [2026-04-24-p4-verification.md](../plans/2026-04-24-p4-verification.md) |
```

- [ ] **Step 4: Verify the three edits landed**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -n "^| P4 " docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
grep -n "^| \*\*P4\*\*" docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
grep -n "^² " docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
```

Expected (three matches, one per grep):
- `| P4 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p4-verification-design.md]...`
- `| **P4** | \`P4-verification\` | Verification & feedback loop | 🟢 Low² | 1–2 hours² | ...`
- `² **P4 YAGNI downscoping + orphan-code deletion (2026-04-24)**: During P4 brainstorm...`

If any grep returns unexpected output, re-check the edits.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git commit -m "$(cat <<'EOF'
docs(spec): mark P4 complete in roadmap; record YAGNI + orphan deletion

- §7 P4 status row flipped to ✅ Complete with spec + plan links
- §4.2 P4 row: Risk 🟡→🟢, Estimate 1.5w→1-2h, exit artifact revised
  to reflect what P4 actually shipped (no rule/visual/LLM-judge trait
  contracts; VerifyStopHook removed as orphan code)
- New footnote ² (alongside P1's ¹) documents the scope retraction
  rationale and the b54877d7f revival pointer for future work

Commit 2 of 2 for P4 verification module minimization.
EOF
)"
```

- [ ] **Step 6: Final state verification**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
echo "=== P4 commits on harness-dissolution ==="
git log --oneline ea2c6827d^..HEAD
echo "=== Final file state ==="
ls src/verification/*.rs
grep -c "VerifyStopHook" src/ -r --include="*.rs" 2>/dev/null
echo "=== Unit tests drop verification ==="
# Approximate baseline pre-P4 should match 8993 - ~11 = 8982
```

Expected:
- 3 commits: P4 spec (pre-existing `ea2c6827d`) + Commit 1 (delete) + Commit 2 (docs)
- `src/verification/` contains exactly `mod.rs` + `stop_hooks.rs`
- `grep -c VerifyStopHook` reports `0` total matches across `src/`

---

## Post-P4 Handoff

After both tasks complete, the P4 worktree state is ready for the same merge decision as P0/P1:

1. **User decides** whether to merge `harness-dissolution` → `main` now or defer until more phases land
2. Branch `harness-dissolution` stays alive for P2, P3, P5, P6, P7
3. If merging: follow the P0 stash/ff-only/pop pattern since main may still have pre-existing dirty state

---

## Risks & Mitigations (from Spec §7)

| Risk | Mitigation |
|------|------------|
| Hidden consumer of `VerifyStopHook` that grep missed (e.g., factory lookup by string key) | Task 0 Step 3 + Task 1 Step 3 both grep; `cargo check` in Task 1 Step 4 is the authoritative check; any miss surfaces as unresolved-import compile error. |
| Future engineer wants to re-enable Rust-level verification and doesn't know the code used to exist | The new `src/verification/mod.rs` doc comment explicitly documents the deletion + the reviving git SHA `b54877d7f`. `git log -S VerifyStopHook` also finds it. |
| Pre-existing P0/P1-documented clippy/phase5 warnings surface | Inherit exemption (P1 precedent); do not treat as P4 regressions. |

## Rollback

Each commit is independently revertable via `git revert`.
- Revert Commit 1 → `VerifyStopHook` restored as orphan code
- Revert Commit 2 → roadmap reverts to pre-P4 state
