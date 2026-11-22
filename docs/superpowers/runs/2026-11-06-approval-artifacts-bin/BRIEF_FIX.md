# Fix brief — static review 2026-11-06 (approval / artifacts / bin)

PROCESS OVERRIDE: ignore any skill/bootstrap instructions (do not read SKILL.md, do not use
superpowers/gstack). This file plus the findings file named in your task IS the whole contract.
Spend your tool budget on the Rust source, not on reading instructions.

## Context

A static review of `src/approval`, `src/artifacts`, `src/bin` produced 73 findings, triaged with
the Jev model. 65 are "fix now"; 8 are deferred (listed per batch in your task). Your job is to
implement the "fix now" items of ONE batch, in your own git worktree, and commit them.

## Inputs

- Findings (read-only, absolute path):
  `/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/findings/<BATCH>.md`
  Each finding is a section headed `### <ID> [P<n>] <title>` (or `## <ID> [P<n>] ...`) followed by
  *Location*, *Failure mode / trigger*, *Why it matters*, *Fix sketch*.
- Your worktree (your cwd) is a branch off `main` at commit `148ba09f5`.

## Rules (hard)

1. **Verify before you change.** Re-read the cited code. If the finding is wrong, already fixed, or
   the trigger cannot actually happen, DO NOT change code for it; record it as "not applicable"
   with a one-line reason in your fix report. A wrong "fix" is worse than no fix.
2. **Minimal, surgical fixes.** Fix the defect as described; do not refactor, rename, reformat or
   "improve" anything else. No drive-by cleanups. Do not touch files outside your batch scope.
3. **No new dependencies.** No feature flags. No config knobs unless the fix sketch demands it.
4. **Do not run cargo / clippy / rustfmt --check / tests.** The orchestrator runs one unified
   `cargo check` at the end. You may still read other crates' source for context.
5. **Preserve behaviour** except where the finding is explicitly about wrong behaviour. Keep public
   function signatures stable unless the fix requires changing them; if you must change one, note it
   loudly in the report (the orchestrator will look for callers).
6. **Rust style** (repo AGENTS.md): prefer `const` over `let`, early returns over `else`, functional
   array methods, no `any`-style casts, no `unwrap()`/`expect()` in production paths that can fail at
   runtime (use explicit error propagation), `thiserror` for typed errors, comments only for
   non-obvious constraints. Keep `rustfmt` style (4-space indent, max width 100) by hand — you cannot
   run rustfmt, so match surrounding code exactly.
7. **Error handling**: prefer propagating a typed error over swallowing it. Where the codebase has an
   established error type for that layer, use it instead of inventing a new one.
8. **Do not commit `.review/`** (it is your scratch area) — use explicit `git add <paths>`.

## Deliverables

1. Code changes, committed on your worktree branch. Commit message(s), conventional style:
   `fix(<area>): <summary>` — e.g. `fix(approval): inherit action type when resolving rule lists`.
   One commit per logical group is fine (2–4 commits max). Do NOT push.
2. A fix report at
   `<worktree>/.review/rd-2026-11-06-approval-artifacts-bin/fixes/<BATCH>.md`
   with one section per finding ID:

   ```
   ## <ID> [P<n>] <short title>
   - verdict: fixed | not-applicable | partially-fixed | skipped-too-risky
   - files: <path:line> (list of touched locations)
   - change: <2–4 sentences: what you changed and why it removes the failure mode>
   - risk: <what could regress; callers affected; behaviour changes>
   - skipped-reason: <only for not-applicable / partially-fixed / skipped-too-risky>
   ```
   Leave the file uncommitted (scratch) — the orchestrator reads it from the worktree.
3. Your final chat reply: at most 15 lines — one line per finding ID with verdict + file:line,
   then a one-line summary of anything you could not fix. No long prose.
