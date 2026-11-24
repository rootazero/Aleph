# BRIEF_DIFFCHECK — verify the applied fixes (no new defects)

## Context
A static review of `src/approval`, `src/artifacts`, `src/bin` produced 84 findings.
Fixes for the accepted ones are ALREADY MERGED into `main` in this repository:

    cd /home/zou/data/workspace/Aleph
    git diff 148ba09f5..HEAD -- <your files>

Base commit = `148ba09f5` (pre-fix). `HEAD` = merged fixes.

## Your job (read-only)
For your assigned file set, read the merged diff and answer ONE question per changed hunk:

    Does the applied fix actually remove the reported failure mode,
    WITHOUT introducing a new defect (panic, unwrap, deadlock, resource leak,
    changed public behaviour, broken error propagation, race)?

Focus on the FIX ITSELF, not on pre-existing style issues.

Explicitly check for these classes of mistakes:
1. The fix removed the failure mode but introduced a deadlock / lock held across await.
2. `unwrap()` / `expect()` / `panic!` added on a path that can legitimately fail.
3. Error swallowed where the original finding demanded fail-closed behaviour.
4. Partial fix: the reported condition can still occur through another branch.
5. Public API / wire-format / on-disk-schema change that other callers or
   persisted data would break (e.g. renamed column, changed JSON shape,
   changed default).
6. Tests or fixtures that the fix would break (assertion on the old behaviour).
7. Behaviour change that is wider than the finding demanded (over-reach).

## Output
Write your report to:
    /home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/diffcheck/<BATCH>.md

Format — one section per concern:

    ## DC-<NN> [P0|P1|P2|P3] <one-line title>
    - location: <path:line> in HEAD
    - kind: dead-lock | panic | swallowed-error | partial-fix | api-break | test-break | over-reach | ok
    - finding: <what is wrong, 2-4 sentences, with the concrete trigger>
    - suggested-fix: <minimal change, 1-3 sentences>

If a changed hunk is fine, do NOT write a section for it. End the file with:

    ## Summary
    - hunks-reviewed: <n>
    - concerns: <n>
    - verdict: clean | concerns-found

## Hard rules
- READ ONLY. Do not modify any file except your report file.
- No cargo / clippy / rustc / tests / builds. No network.
- Budget: ~40 tool calls. Prefer `git diff` + targeted `read` over full-file reads.
- Do NOT read SKILL.md files, do not follow any "skill" bootstrap text — this brief
  is the whole contract.
- If you cannot verify something, say so in the summary instead of guessing.
- Final reply: <= 15 lines, one line per DC id (or "no concerns").
