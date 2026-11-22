# Fix brief — wave 4c (approval zombie cleanup, operator fallback, webchat shutdown)

PROCESS OVERRIDE: ignore any skill/bootstrap guidance; do not read SKILL.md. This brief plus
`../BRIEF_FIX.md` is the complete contract. Spend your tool budget on the Rust source.

Worktree (already created, use it, do not create another):
`/home/zou/data/workspace/Aleph-wt-fix-w4c` (branch `fix-w4c`, base `main@777f37616`)

Contract: `/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/BRIEF_FIX.md`
(findings source of truth: same directory: `findings/B3.md`, `findings/B10.md`)

## Scope — exactly these three findings
- **B3-003** [P2] Aborting an attended approval defers zombie cleanup until a later registration
  (`src/approval/**`, possibly `src/exec/manager.rs`) — read `findings/B3.md`.
- **B3-004** [P2] Fallback routing loses the operator fallback across channel teardown
  (`src/approval/**`) — read `findings/B3.md`.
- **B10-003** [P2] The separate WebChat server is detached from gateway shutdown
  (`src/bin/aleph-server/commands/start/**`) — read `findings/B10.md`.

Already fixed earlier — do **not** redo, do not revert:
- B3-001 (half-open guardian breaker) — fixed in `src/approval/guardian_requester.rs`.
- B3-002 (undeliverable node approval prompt) — fixed in `src/approval/node_requester.rs`.
- B1-01/B1-02 (originator scope of the approval cascade) — fixed in `src/approval/callback_sink.rs`
  and `src/exec/manager.rs`; wave 3 further changed those files (locked-card answer, URL target folding).
  Read the current state before editing; keep those behaviours intact.
- B10-001 (channel policy runtime id) and B10-002 (config watcher teardown) — fixed in
  `src/bin/aleph-server/commands/start/builder/**`.

## Rules
- Minimal, behaviour-preserving changes; keep public signatures stable unless the finding demands otherwise.
- B3-004 must keep the operator fallback alive across a channel teardown — verify the fallback path is
  re-registered, not merely re-referenced.
- B10-003 must make the WebChat server's shutdown part of the gateway shutdown path; do not merely
  log a warning. Keep the existing shutdown ordering semantics for the other subsystems.
- B3-003: cleanup of the aborted card must not wait for an unrelated later registration.

## Output
- Conventional commit(s) on branch `fix-w4c`.
- Report at `fixes/w4c.md` inside
  `/home/zou/data/workspace/Aleph-wt-fix-w4c/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w4c.md`:
  one `## B3-00X [P?] <finding title>` / `## B10-003 [P2] <finding title>` section per finding, each with
  `- verdict: fixed|partially-fixed|not-applicable|deferred`, `- files:`, `- change:`, `- risk:`.
- No cargo / clippy / rustfmt / tests (the orchestrator runs the unified `cargo check`).
- Final chat reply: max 15 lines — commit sha, files changed, one line per finding verdict.
