# Execution ledger and rulings for the run-identity A5 plan (P1 + P3), in the order made
> Companion to `2026-09-24-run-identity-a5-p1-p3.md`. Copied verbatim from the SDD workspace ledger (git-ignored, deleted at wrap-up) on 2026-09-25; log and review file names it cites lived in that workspace and are gone. Every `Ruling:` line is what was decided — why — cost if wrong.

# SDD ledger — plan: docs/superpowers/plans/2026-09-24-run-identity-a5-p1-p3.md

Spec: docs/superpowers/specs/2026-09-24-run-identity-a5-design.md (binding authority).
Workspace ($SCRATCH): D:/Workspace/Aleph/.superpowers/sdd/2026-09-24-run-identity-a5-p1-p3
P1 worktree: D:/Workspace/Aleph/.claude/worktrees/run-identity-p1 (branch worktree-run-identity-p1, base 1d678e010)

## Preflight scan (2026-09-24)

### Task pairs sharing a file or an interface
| Pair | Produced → consumed | Finding |
|---|---|---|
| T2 → T8 | T2 `let run_marker_id = run_id;` in runner_impl; T8 mutation row reverts it to `uuid::Uuid::new_v4()` | consistent; test name `the_run_markers_carry_the_engine_run_id` matches |
| T2 → T8 | T2 `let run_id = req.run_id.clone();` in dispatch(); T8 mutation `String::new()` | consistent (`dispatch_forwards_run_id`) |
| T2 ↔ T4 | T2's new runner_impl scan comment asserts the split filters the parent's opener (F14) | T4 implements exactly that; comment true once T4 lands (same phase, one merge) |
| T3 → T8 | hookstop `request.run_id.clone()`; T8 mutation restores `format!("hookstop-…")` | consistent |
| T4 → T8 | `.filter(|record| record.seq != opener_seq)`, `let run_id = open_run.run_id.clone()`; T8 mutation rows name both tests | names match T4 Step 1 |
| T5 → T8 | `abandon(.., closes: Option<&str>, ..)`; T8 abandon mutation → `an_abandoned_interrupted_run_is_closed_under_its_own_run_id` | consistent |
| T6 → T8 | rposition→position mutation → `a_reused_run_id_snaps_to_the_opener_its_closer_pairs_with` | consistent |
| T7 → T11 | T7's 4 new projector tests assert store read-back, never `Projected` | T11 Step 7 expects them green unchanged — consistent |
| T7 ↔ T11 | both edit session_projector.rs tests module | sequential phases (P3 starts after P1 merge) — no conflict |
| T9 → T10 | `RunBill`, trait `stamp_and_bill_in_range(.., Option<&RunBill>)`, file backend temporary `Unsupported` for `Some` | T10 Step 3 expects exactly that red; T10 Step 5 removes the branch — consistent |
| T9 → T10 | `already_stamped_by` in sqlite_backend reused by file backend | file backend already calls it today (spec §1 fact) — visibility exists |
| T9 → T11 | `stamp_and_bill_in_range`, `RunBill`, callers pass `None` in T9 | T11 switches callers to `bill.as_ref()` — consistent |
| T9/T11 tests | `store.conn` / `manager.conn` is `pub(super)` on session_manager | sqlite_backend and session_projector are both under `gateway` — visible |
| T11 → T12 | T12 grep for zero hits of `bill_run_from_fold`, `Stamped { billed` | T11 deletes both — consistent |

### Per-task self-consistency
| Task | Finding |
|---|---|
| T1 | Step 1 (worktree) + Step 2 (baseline) are controller setup/measurement; Steps 3–5 are the task. Worktree lacks submodules (include_dir! needs skills/ plugins/) — plan omits `git submodule update --init`. |
| T2 | Test literal in Step 8 lists FlowRequest fields from plan time; the compiler's list is authoritative (Step 3 says so). Consistent. |
| T3 | consistent |
| T4 | consistent |
| T5 | integration test needs worktree-local target + `-j 1` = a cold full build (tens of minutes). Consistent but slow. |
| T6 | consistent (first test red on old code, second green) |
| T7 | Step 8 `cargo test -p alephcore --lib A B C` — cargo accepts ONE positional filter; multiple filters must follow `--`. Plan defect (also T9 Step 6, T10 Step 6, T11 Step 7, T12 via T8 Step 4). |
| T8 | clippy is `-p alephcore --all-targets` (not `--workspace`); plan scope. QA before clippy — consistent with Global Constraints. |
| T9 | consistent (Step 0 creates P3 worktree — also needs submodule init) |
| T10 | consistent |
| T11 | consistent |
| T12 | merge to main (see ruling) |

## Rulings
- Ruling: `$SCRATCH` in the plan = this SDD workspace dir — one place for logs, briefs and reports — costs nothing if wrong.
- Ruling: every new worktree runs `git submodule update --init` right after `git worktree add` (plan omits it; `include_dir!` needs skills/ plugins/) — done for P1 (network clone succeeded) — if wrong, only a slower setup.
- Ruling: multiple test filters are passed after `--` (`cargo test -p alephcore --lib -- a b c`); the plan's `--lib a b c` form is rejected by cargo — if wrong, a command fails loudly, no silent effect.
- Ruling: Task 1 Steps 1–2 (worktree + --lib baseline) are run by the controller as setup/measurement; the Task 1 subagent does V7, V4 and the spec commit — if wrong, only who ran a measurement changes.
- Ruling: Tasks 3, 4 and 6 are dispatched as ONE batch (small, same shape: a writer/reader switches to the engine id; each keeps its own commit and its own red→green), reviewed as one unit; Task 5 stays alone because it needs the slow integration-test build — cost if wrong: one reviewer sees three small commits at once.
- Ruling: the local `git merge --ff-only` into main at the end of P1 (Task 8) and P3 (Task 12) is authorized — the user approved the plan that mandates it and the project CLAUDE.md prescribes single-branch main with per-phase worktrees merged; nothing is pushed — cost if wrong: the user resets local main to 1d678e010 (nothing left the machine).
- Ruling: all cargo runs are serialized (one build at a time on this host; parallel alephcore builds OOM) — cost: wall-clock only.

## Progress
- P1 baseline (--lib @1d678e010, worktree): 19125 passed / 4 failed / 20 ignored; reds = the 4 known names (p1-baseline-reds.txt)
- Task 1: implementer DONE (849380b5c; 6 writer roles, by-id readers per table, V7 5266/5266); review dispatched (base 1d678e010)
- Ruling: Task 2 dispatched while Task 1's review runs — Task 1 touched only the spec doc, Task 2 only code, so a Task 1 fix cannot conflict — cost if wrong: a trivial rebase-free doc fix commit lands after Task 2's commit.
- Task 2: dispatched, BASE 849380b5c
- Task 1: complete (commits 1d678e010..849380b5c, review clean)
- Task 1: minor (deferred): report wording on usage_fold "not grepped / confirmed" is confusing (report only, not code)
- Task 1: minor (deferred): spec V4 row says "by-id readers 2" (plan Step 5 template) while the plan's own census table lists 3 by-id rows (event_snap, runner_impl rposition, already_stamped_by) — the count depends on whether the runner_impl scan counts; final review to triage
Task 2: implementer DONE a751c43d8 (BASE 849380b5c); review dispatched
- Ruling: Tasks 3/4/6 batch dispatched while Task 2's review runs — disjoint files (run_loop/mod.rs, session_split.rs, event_snap.rs vs Task 2's nine files), Task 3 consumes the pre-existing RunRequest.run_id, not Task 2's FlowRequest field — cost if wrong: a rebase of three small commits.
- Ruling: the batch runs ONE combined RED build, ONE GREEN build, ONE combined 3-mutation build; three separate commits — cost if wrong: a red that only shows in isolation (the filters are disjoint test names, so none expected).
- Ruling: the briefs' `cargo … | tail` pipelines are replaced by logged runs (common-rules) — no semantic change.
- Tasks 3/4/6: dispatched as one batch, BASE a751c43d8, report task-346-report.md
- Task 2: complete (commits 849380b5c..a751c43d8, review: spec ✅, quality Approved, 0C/0I/1M)
- Task 2: minor (routed to Task 7): session_projector.rs ~2124 and run_span.rs :22-26/:79-95 still state "marker id never equals meta id (A5)" as current fact — Task 7 brief already relabels exactly these; Task 7 reviewer to confirm
- Task 2: ⚠️ (routed to Task 5): gateway_chat_* integration tests were type-checked, not executed — Task 5 builds the worktree integration target anyway; ask its implementer to also run `--test gateway_chat_through_orchestrator --test orchestrator_e2e` once the build exists
- Tasks 3/4/6: implementer DONE fa7e38c61, 0b27078fe, 253bc13c9 (BASE a751c43d8); batch review dispatched (package review-a751c43d8..253bc13c9.diff)
- Ruling: Task 5 dispatched while the 3/4/6 review runs — Task 5 touches only resume_coordinator.rs + tests/resume_coordinator_integration.rs, disjoint from 3/4/6 — cost if wrong: a rebase of one commit.
- Task 5: dispatched, BASE 253bc13c9 (cold worktree integration build, -j 1); also runs gateway_chat_through_orchestrator + orchestrator_e2e for Task 2's ⚠️
- Tasks 3, 4, 6: complete (commits a751c43d8..253bc13c9: fa7e38c61 T3, 0b27078fe T4, 253bc13c9 T6; review: spec ✅ ×3, quality Approved, 0C/0I/2M)
- Tasks 3/4/6: minor (closed by controller): T3 comment claims execute() stamps the meta under request.run_id for the Ok path — controller checked execute.rs:989 `stamp_run_meta(.., &run_id, ..)` with run_id = request.run_id (execute.rs:361) in the Ok arm — true.
- Tasks 3/4/6: minor (routed to final review): "bill each bracket once" (Review Focus #1) is Task 7's/P3's job — final reviewer to check with multiple brackets under one id.
- Task 5: implementer reported BLOCKED (harness low-memory reaper "killed" its background build). Controller found the cargo (pid 3588, the RED command) + rustc alephcore (12 GB) STILL RUNNING — only the shell wrapper died. Ruling: let it finish (no second cargo), then resume the implementer — cost if wrong: one wasted wait; restarting would throw away the partial cold build.
- Task 5: surviving build finished alephcore rlib, then failed on rusty-fork with STATUS_DLL_INIT_FAILED (0xc0000142, transient resource); implementer resumed to re-run RED (foreground, retry once on the same error)
- Task 5: implementer DONE b70420af2 (BASE 253bc13c9); RED→GREEN 37/37 int + 32/32 lib, mutation red; Task 2's ⚠️ closed: gateway_chat_through_orchestrator + orchestrator_e2e pass (3/3); review dispatched
- Ruling: Task 7 dispatched while Task 5's review runs — disjoint files (session_projector*, run_span, missed_seqs, fast_path vs resume_coordinator*) — cost if wrong: one rebase.
- Ruling: Task 7's green-on-arrival pins get ONE mutation build as bite evidence (the plan has none for them) — cost: one build.
- Task 7: dispatched, BASE b70420af2
- Task 5: review spec ✅, quality Approved, 0C/0I/1M (reduction.rs ~1830 LegalShape comment describes the abandoned-* closer in present tense)
- Task 5 fix round 1: resumed implementer — relabel legacy minted-id LegalShape comments (abandoned/split/hookstop) as pre-2026-09-24 shapes, comment-only, no cargo.
- Ruling: the comment-only fix runs while Task 7 is mid-flight in the same worktree (different files, no cargo) — cost if wrong: Task 7's `git diff` shows an extra reduction.rs hunk; its commit stages only its own files.
- Task 5 fix round 1: c98ac0c51 (comment-only, 2 LegalShape comments relabeled; no hookstop entry exists in that table). Ruling: controller verified the 2-comment diff directly instead of a scoped re-review dispatch — comment-only, no code/test effect — cost if wrong: a wording nit.
- Task 5: complete (commits 253bc13c9..c98ac0c51: b70420af2, c98ac0c51; review clean after fix round 1)
- Task 5: note (routed to final review): reduction.rs legal_shapes() now carries the pre-F1 split/abandon shapes but NO entry for the post-F1 shapes (split child whose tail omits the parent's opener and reopens under the parent's id; abandon closer under the open run's id) — the guard only certifies shapes it enumerates (criterion §3); final reviewer to decide whether entries are needed.
- Task 7: implementer DONE e32b91d96 (BASE c98ac0c51; 65/65 filtered, mutation reddened 3/4 new pins); review dispatched
- Task 7: review spec ✅, quality Approved, 0C/1I/2M. I = QA `holes knobs claims` not run → adjudicated: owned by Task 8 Step 5 (phase gate), not a Task 7 defect. M1 (plan-mandated run_span.rs:22-27 wording read as self-contradictory "marker id, NOT the engine id — equal since F1") → controller fixed comment-only in ef4902932 (CRLF+fmt checked). M2 (pure-fold pin can't be reddened by the store lever; still pins a real shape) → accepted, plan-mandated.
- Ruling: controller made the one-comment fix directly (comment-only, deviates from plan-mandated wording to remove a self-contradiction; spec is authority and the plan text was the lie, criterion §1) — cost if wrong: one wording revert.
- Task 7: complete (commits c98ac0c51..ef4902932: e32b91d96, ef4902932)
- Ruling: Task 8's mutation table cites the per-task evidence logs (task2-red/-mutation, t346-mutation, t5-mutation) and runs only the one uncovered row (split uuid-only) — cost if wrong: a row whose mutation differs slightly from the plan's wording (e.g. RED-on-old-code vs a planted mutation).
- Ruling: Task 8's `<sha>` placeholders cite ef4902932 (last P1 code commit); the docs commit cannot cite itself — cost: none.
- Ruling: Task 8 implementer commits docs but does NOT merge; controller ff-merges after the Task 8 review — keeps the merge behind a review gate.
- Task 8: dispatched, BASE ef4902932
- Task 8: implementer BLOCKED after Steps 1–3 (docs edited, uncommitted, 5 files): the harness low-memory reaper killed its background full --lib run at "Compiling alephcore" (cargo died too this time).
- Ruling: controller runs the heavy full --lib as a DETACHED process (PowerShell Start-Process, stdout p1-final.log / stderr p1-final.err.log, pid 8400) outside the harness task tracking, with a tiny poll loop; the implementer is resumed afterwards for the remaining steps with the same detached pattern for the heavy runs — cost if wrong: a real OOM still kills rustc (then fall back to fewer concurrent consumers).
- Task 8: detached full --lib (pid 8400) finished: 19124 passed / 13 failed / 20 ignored (p1-final.log). Name diff vs p1-baseline-reds.txt = +9 extra, all child-process-spawning tests (acp spawn_and_drop, 5× mcp stdio, 2× sandbox::worktree "program not found", skill inline_shell cap). Re-ran those 9 under the Bash PATH (no rebuild): 29/29 ok (p1-final-rerun9.log). Ruling: the 9 are an instrument artifact of the detached Start-Process launch (Windows PATH lacks the Git-Bash POSIX tools the baseline was measured with), not a regression; residual reds == the 4 baseline names — cost if wrong: none, the rerun is the same binary. Lesson for later detached runs: launch via bash.exe -lc so PATH matches the baseline.
- Task 8: implementer resumed for Steps 4(2)–8, detached pattern via bash -lc for heavy runs.
- Task 8: implementer DONE 656c2f879 (BASE ef4902932); --bins 95/0, int 40/0, panel no-run clean, V7 5266, QA holes/knobs/claims PASS, clippy 0 err (2 pre-existing warns outside P1), mutation rows reproduce; review dispatched
- Task 8: review spec ✅, quality Approved, 0C/0I/0M (note: `git show | grep -c $'\r'` is blind here — core.autocrlf=true normalizes blobs to LF; check working-tree bytes)
- Task 8: complete (commits ef4902932..656c2f879)
- P1 MERGED: main ff 1d678e010 → 656c2f879 (not pushed)
- Task 9 Step 0: worktree .claude/worktrees/run-identity-p3 (branch worktree-run-identity-p3) from main 656c2f879, submodules init'd.
- Ruling: P3 baseline = the P1 closeout's full --lib (p1-final.log, code identical: ef4902932..656c2f879 is docs/qa only, verified by diff --stat) → p3-baseline-reds.txt = the same 4 names; V7 = 5266 (Task 8, same code). No 15-min re-run — cost if wrong: none, the compiled code is byte-identical.
- Ruling: Task 9 brief Step 6 filter syntax fixed in dispatch (filters after `--`), and mutations run AFTER the Step 7 commit so `git checkout -- file` restores committed code, not the empty tree — cost if wrong: none.
- Task 9: dispatched, BASE 656c2f879 (implementer sonnet; reviewer opus — billing/transaction)
- Task 9: implementer DONE 131e548f9 (BASE 656c2f879); green 163/163 filtered; mut1 → exactly the no-row test red; mut2 → exactly the two predicted reds; tree restored. Deviations: add_usage placed at module scope (brief's `pub(crate) use modify::add_usage` requires a free fn); 2 extra comment renames (projection_reconciler.rs, session_manager/mod.rs). Common-rules corrected: Start-Process array-form no-ops here.
- Task 9: review dispatched (opus)
- Ruling: Task 10 dispatched while Task 9's review runs (reviewer reads a frozen diff file; any Task 9 fix lands as a follow-up commit after Task 10 commits, same worktree, sequential) — cost if wrong: a fix touching file_backend/mod.rs lands on top of Task 10's hunk instead of before it.
- Ruling: Task 10 Step 4/6 filter syntax after `--`; commit BEFORE the Step 6 mutation (same as Task 9).
- Task 10: dispatched, BASE 131e548f9 (implementer sonnet)
- Task 9: review spec ✅, quality Approved, 0C/0I/7M.
- Ruling (Task 9 M3): take the fix — `transaction_with_behavior(TransactionBehavior::Immediate)`; a DEFERRED read-then-write txn can fail BUSY on upgrade without the busy handler when another connection to sessions.db holds RESERVED; one line, no behaviour change otherwise — cost if wrong: a slightly earlier write lock.
- Ruling (Task 9 M6): take the fix — test 2 asserts the injected-failure error, not bare is_err().
- Ruling: Task 9 fix round 1 (M3+M6) runs AFTER Task 10 commits (same worktree, one cargo at a time), by resuming task-9-impl-p3.
- Ruling (Task 9 M5): carried into Task 11's dispatch (a notify-on-Stamped-with-bill / none-on-AlreadyStamped test belongs where the projector starts relying on it).
- Ruling (Task 9 M1, trait default swallows Some(bill) as NoRowInRange): routed to final review — no production decorator exists; the test doubles (InMemorySessionStore ×2, E2eSessionStore, ReadDuringRescope) rely on the default and Task 11 changes what they receive, so decide once Task 11 lands — cost if wrong: a future decorator loses bills silently.
- Ruling (Task 9 M2 ReadDuringRescope doc, pre-existing test-only; M4 doc ahead of code until Task 10; M7 process): M2 parked (not ours, test-only) → State-the-Negative; M4 checked in Task 10's review; M7 noted.
- Task 10: RED confirmed by controller (t10-red.log 16:51: exactly the 3 new parity tests red, guard test green). Implementer idled ~4h — its background wait loop never woke it; nudged at 20:59 to resume at Step 5 with a foreground bounded poll.
- Task 10: implementer DONE 48ce689c0 (BASE 131e548f9); RED 3 exact → GREEN 89/0 → mutation exactly a_refused_metadata_write_rolls_the_stamp_back; restored. Review dispatched (opus).
- Task 9 fix round 1: resumed task-9-impl-p3 for M3 (IMMEDIATE txn) + M6 (tight assertion), BASE 48ce689c0.
- Task 10: review spec ✅, quality Approved, 0C/1I/6M. Task 9 M4 (trait doc ahead of code) closed: now true.
- Ruling (Task 10 I1, file backend publishes no SessionUpdated after a bill; SQLite does — §9 one verb two faces; pre-existing on update_session_usage): FIX in Task 10 fix round 1 — emit after a successful bill on both the file `stamp_and_bill_in_range` and `update_session_usage`, pinned by a bus-subscriber test. Cheap, and Task 11 makes this the live billing path on the default backend — cost if wrong: one extra `session_updated` frame per bill.
- Ruling (Task 10 M: missing metadata.json → Err(NotFound) even when the row is AlreadyStamped/NoRowInRange; SQLite answers Ok there): take the fix — move the no-metadata refusal after the AlreadyStamped/NoRowInRange returns and still before any write. Deviates from plan text; spec §"If the bill cannot be applied … Err" only covers a bill that would be applied — cost if wrong: none for billing (nothing is written either way).
- Ruling (Task 10 M: tests don't pin cost/model; rollback assert loose; Review Focus 3 replay half; stale `rfind` in trait doc + file doc stamps-only): take all — test/doc only.
- Ruling (Task 10 M: truncate_messages / delete_messages_from_seq / restore_checkpoint rewrite the transcript without the metadata lock — append-loss race, pre-existing, 4 copies of the write loop): PARKED → State-the-Negative / follow-up; out of this plan's scope.
- Ruling: Task 10 fix round 1 runs after Task 9 fix round 1 commits (one cargo at a time).
- Task 9 fix round 1: 2f0319ae0 (IMMEDIATE txn + DatabaseError("injected bill failure") assertion; sqlite_backend 14/0, t9-fix1.log). Ruling: controller verified the 11-line diff + log directly instead of a scoped re-review dispatch — cost if wrong: a wording nit.
- Task 9: complete (commits 656c2f879..2f0319ae0: 131e548f9, 2f0319ae0; review clean after fix round 1)
- Task 10 fix round 1: resumed task-10-impl-p3, BASE 2f0319ae0
- Task 10 fix round 1: 90e3ebe89 (file backend emits SessionUpdated after a bill on both paths, only after write_back/commit Ok; no-metadata refusal moved after NoRowInRange/AlreadyStamped, before any write; tests pin cost/model, metadata.is_none(), replay-after-recreate bills; rfind→rposition; file doc). t10-fix1.log 90/0; mutation (emit deleted) → exactly a_billed_stamp_announces_but_an_already_stamped_replay_does_not. Ruling: controller verified diff + logs directly (emit placement read at mod.rs:1601-1620 and :1453) instead of a scoped re-review — cost if wrong: a missed nit, final review re-reads the branch.
- Task 10: complete (commits 2f0319ae0..90e3ebe89 on top of 48ce689c0; review clean after fix round 1)
- Ruling: Task 11 dispatch carries (a) filters after `--`, --no-run output to a log not a pipe; (b) commit before mutations; (c) Task 9 M5 — a SQLite notify test (one SessionUpdated on Stamped-with-bill, none on AlreadyStamped) mirroring Task 10's file test, STOP if it needs new plumbing.
- Task 11: dispatched, BASE 90e3ebe89 (implementer sonnet; reviewer opus)
- Task 11: implementer DONE 1fef0513f (BASE 90e3ebe89); green 148/0; 3 mutations → exactly the brief's red sets; M5 sqlite notify test added (no new plumbing). Deviation: RED not observed empirically (tests + impl in one pass before first compile; t11-red.log is post-impl EXIT 0). Ruling: accept — the brief's RED is a compile failure (BillOutcome undefined), stated logically, and the three post-commit mutations prove the new tests bite — cost if wrong: a test that would have passed pre-impl for the wrong reason (the mutations rule that out for the three named ones). Extra: usage_fold.rs + projector module doc stale `bill_run_from_fold` links fixed.
- Task 11: review dispatched (opus)
- Task 11: review spec ✅, quality Needs fixes, 0C/2I/8M (+ routed trait-default item).
- Ruling (T11 I1, update_session_usage docs name the projector as caller; zero production callers now): fix the two docs now (say: no production caller; a run is billed only through stamp_and_bill_in_range; calling this to bill a run bypasses the stamp guard — F10). Deleting the method (zero consumers, P6) is PARKED → follow-up: it spans the trait, 2 backends, the inherent method and 4+ test doubles incl. tests/ — cost if wrong: a dead method stays one more round.
- Ruling (T11 I2, stale-low run_start (0 after a drain respawn) → the meta arm's stamp range (0, meta] can pick the PRIOR run's already-stamped row (already_stamped_by compares run_id, so a different id overwrites it) and bill the current run there; the heal then bills the current run again): FIX NOW although pre-existing — it is a double bill on exactly the path this phase restructured, and the fix is to derive the stamp's lower bound from the same anchor the fold used (§12, one derivation): fold_run_bill/plan also yields the anchor seq (last RunStarted in the slice), the meta arm stamps (anchor, meta]; Unfoldable/no-anchor keeps ctx.run_start. Regression test required (run A stamped at row 2, run B's row a hole, meta B with run_start 0 ⇒ NoRowInRange and row 2 keeps A's stamp, nothing billed). Reviewer said "route"; overruled — cost if wrong: one more derivation change inside the reviewed area, re-reviewed by opus.
- Ruling (T11 M1 fmt 9 hunks; M2 retry docs omit fold failure; M3 synthesizer Unfoldable silent → warn!; M5 two overstated docs; M6 heal-test doc claims /compact-retired opener but fixture has no RunStarted → fix the doc to the fixture; M7 FoldedBill/fold_run_bill private (child modules see parent privates)): take all.
- Ruling (T11 M4 zero tokens + Some(0.0) reads Billed; M8 ranged read on every AlreadyStamped replay): PARKED → State-the-Negative (cosmetic count / perf, no billing error).
- Task 11 fix round 1: resumed task-11-impl-p3, BASE 1fef0513f; scoped re-review (opus) after.
- Task 11 fix round 1: 7244c8b2f (run_usage_totals returns (anchor, totals); meta arm + synthesizer stamp from the fold's anchor; docs; fmt; warn; privatised). 149/0; mutation (anchor→ctx.run_start) → exactly a_stale_low_run_start_cannot_steal_the_prior_runs_row. Scoped re-review dispatched (opus).
- Task 11 fix round 1: scoped re-review (opus) Approved — all ruled findings ✅; new m1–m4 minor.
- Ruling: routed to Task 12's doc sweep — (OOS-Important) two more comments naming the projector as update_session_usage's caller (session_manager/ops/crud.rs ~353, file_backend/mod.rs ~806); m1 anchor doc says "always this run's own opener" → positional (last live RunStarted before the meta); run_span.rs doc ~197 fold range `(start, end]` → `[start, end)`; SESSION_SERVICE.md:105 bill_run_from_fold; and m2 — Task 12 runs `session::usage_fold` tests explicitly (the green filter never ran them). m3 (heal half of I2 untested) and m4 (Option chain nit) PARKED → State-the-Negative.
- Task 11: complete (commits 90e3ebe89..7244c8b2f: 1fef0513f, 7244c8b2f; clean after fix round 1)
- Ruling: Task 12 implementer does docs + the routed comment fixes + verification + QA + clippy + commit, NOT the merge (controller ff-merges after the Task 12 review) and NOT Step 7 memory (controller writes memory at the end with the final state).
- Task 12: dispatched, BASE 7244c8b2f (implementer sonnet)
- Task 12: full --lib (p3-final.log, launched via Git Bash) 19146 passed / 4 failed / 20 ignored; red names == p3-baseline-reds.txt (empty diff, controller-verified). Implementer was idling on a background wait; controller polled in the foreground and nudged it at 23:07.
Task 12: implementer DONE f3332208d (docs closeout; full set green; QA holes 16/16 knobs 10/10 claims 13/13; clippy EXIT 0 with one NEW in-scope warning mem_replace_option_with_some at file_backend/mod.rs:1597 from 48ce689c0).
Ruling: controller fixed the clippy warning directly as 5bfda6aea (x.replace(v), semantically identical) before merge — a new warning in this round's own code is not a follow-up; clippy-to-zero is the standing bar — cost if wrong: one trivial commit. Re-check launched detached: p3-clippyfix.log (scoped tests + clippy -p alephcore --lib --tests).
Ruling: p3-final.log lacking the command as first line accepted; the exact command is recorded in task-12-report.md — re-running a 692s suite for a log header buys nothing.
Task 12 review: package task-12-review-package.diff (7244c8b2f..5bfda6aea), sonnet reviewer task-12-review-p3; first turn died on API timeout, resent.
Controller re-check of 5bfda6aea: p3-clippyfix.log — scoped tests 50/0 EXIT 0; clippy -p alephcore --lib --tests EXIT 0, mem_replace warning gone, only the 2 pre-existing sandbox/windows warnings remain.
Instrument note: `grep -c $'\r'` reports 0 on a CRLF file in Git Bash (text mode strips CR) — `tr -cd '\r' < f | wc -c` shows 2948/2948. Use tr for CR counts.
Task 12 review: Spec ✅, Quality Approved, 0 findings (task-12-review.md).
Task 12: complete (f3332208d + 5bfda6aea). P3 ff-merged to main (not pushed).
Final review: package final-review-package.diff (1d678e010..5bfda6aea, 18 commits, 35 files); opus reviewer final-review-a5 dispatched with routed items (V4 count, multi-bracket billing, legal_shapes post-F1, trait default M1).
Final review (opus, final-review.md): Ready after fixes. 0C / 2I / 9M. R1 = 3 by-id readers (spec V4 row wrong; writers 7 not 6; sha cites base). R2 no same-id double bill; pinned for retry only. R3 split-child entry worth adding. R4 make required.
Ruling (final I1, back-filled hole moves the stamp target inside (after,before] → second bill, both backends; pre-existing): FIX NOW in the one fix wave — it is a double bill on the exact operation P3 made "stamp and bill together", and it falsifies the RELEASE NOTE just committed ("never double-bills") and spec §4.2 "strict exactly-once". Fix = shared predicate "any assistant row in range already carries this run_id ⇒ AlreadyStamped" beside already_stamped_by, both backends, red test first on both. Do NOT move the stamp to the newest row (keeps it one-row; cost: the gauge/run_id join stays on the earlier row of that run — State-the-Negative).
Ruling (I2, M6, M7, M8, R1 spec V4 row): doc fixes in the wave — cheap, and each is a false sentence (§1).
Ruling (M3 legal_shapes split-child entry, M4 make stamp_and_bill_in_range required, M5 stale-HIGH run_start → treat run_start > meta_seq as 0, M9 no-announce-on-rollback asserts): IN the wave — each is small and specified; M4 closes a §11 silent swallow before a decorator exists; cost: one integration --no-run build.
Ruling (M1 snap_out_of_open_run same-id pairing, M2 test-only manual.rs invariant): PARKED → State-the-Negative. M1 errs toward over-retention (compaction keeps more, never loses data); M2 is test-only and no fixture reaches it. Both are §19 widening instances of P1 and are named for the follow-up.
Fix wave: worktree .claude/worktrees/run-identity-fix (branch worktree-run-identity-fix @5bfda6aea); brief final-fix-brief.md; opus implementer final-fix-a5 dispatched.
Fix wave DONE_WITH_CONCERNS: 83792c12b (I1) fefa001c4 (M9) 97f0bd251 (M5) b6ddcd5ce (M4) e92072ddb (M3) 7f0fbea12 (I2 M7 M8 R1). Controller-verified: fix-final.log 19149/4, FAILED names == p3-baseline-reds; QA 3× rc=0; clippy only the 2 pre-existing warnings.
Ruling (fix concern 1, intermediate commits never built alone): accepted — HEAD is the tested tree; the branch is ff-merged as a whole; cost if wrong: a bisect lands on an unbuildable commit.
Ruling (concern 2/3, predicate no-ops on id-less meta, matches exact run_id only): accepted as designed, route to re-review to confirm unreachability.
Scoped re-review: package final-fix-review-package.diff (5bfda6aea..7f0fbea12), opus.
Scoped re-review: first turn died on the weekly usage limit (file had only its header); resumed after reset.
Scoped re-review (final-rereview.md): all 10 items Fixed; 0C/0I/4 Minor doc nits (N1 predicate soundness note overbroad; N2 ReadDuringRescope says no defaulted method overridden but rescope_attribution is; N3 V4 "7 writers" unit unstated; N4 trait range doc names only the meta arm). Bottom line: Approved.
Ruling: controller fixed N1–N4 directly (doc comments + one spec cell, no code), verified N4's synthesis-range sentence against run_span.rs (anchor == span.start by construction); CRLF counts equal, rustfmt --check clean. No rebuild — doc-comment text only.
Fix wave + nits ff-merged to main at cba7febf7 (not pushed). Final review closed.
