# Fix brief — wave 4a (aleph-migrate verify.rs)

PROCESS OVERRIDE: ignore any skill/bootstrap guidance; do not read SKILL.md. This brief plus
`../BRIEF_FIX.md` is the complete contract. Spend your tool budget on the Rust source.

Worktree (already created, use it, do not create another):
`/home/zou/data/workspace/Aleph-wt-fix-w4a` (branch `fix-w4a`, base `main@777f37616`)

Contract: `/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/BRIEF_FIX.md`
(findings source of truth: same directory, `findings/B13.md`)

## Scope — exactly these findings
- **B13-10** [P1] filesystem-only sessions still verify as SUCCESS (`src/bin/aleph-migrate/verify.rs`)
- **B13-11** [P1] FTS verification does not check transcript coverage or stale rows
- **B13-12** [P0] `--remove-orphans` deletes attachments still owned by sessions whose directory is missing
- **B13-13, B13-14, B13-15, B13-16, B13-17** — read their sections in `findings/B13.md`; same file

Files you may edit: `src/bin/aleph-migrate/verify.rs` and, only if strictly required for a shared
helper, other files under `src/bin/aleph-migrate/`. Do not touch unrelated modules.

## Rules that matter here
- Verification must **fail closed**: a mismatch between the database and the filesystem has to be an
  error, not a warning, whenever the command would otherwise print SUCCESS / exit 0.
- Orphan cleanup must never delete a file that is still referenced by a live database row, even when the
  owning session directory is missing. Compute the referenced set from the database, not only from manifests.
- Never invent data: report the concrete session id / hash that is missing or unexpected.
- Keep the CLI contract stable (`Migration Status: SUCCESS`, exit codes, flag names). If a check becomes
  fatal, keep the same message shape so operators still understand the outcome.

## Output
- Conventional commit(s) on branch `fix-w4a`, e.g. `fix(migrate): treat filesystem session mismatches as verification errors`.
- Report at `fixes/w4a.md` inside the worktree at
  `/home/zou/data/workspace/Aleph-wt-fix-w4a/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w4a.md`,
  one `## B13-XX [P?] <title>` section per finding you handled, each with
  `- verdict: fixed|partially-fixed|not-applicable|deferred`,
  `- files: <path:line>`, `- change: <what changed and why>`, `- risk: <regression risk>`.
  Keep the section title identical to the finding title in `findings/B13.md`.
- No cargo / clippy / rustfmt / tests (the orchestrator runs the unified `cargo check`).
- Your final chat reply: max 15 lines — commit sha, files changed, and any finding you did not fix and why.
