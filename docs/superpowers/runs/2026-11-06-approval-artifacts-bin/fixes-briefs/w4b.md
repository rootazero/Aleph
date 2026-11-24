# Fix brief — wave 4b (daemon stop must not signal a recycled PID)

PROCESS OVERRIDE: ignore any skill/bootstrap guidance; do not read SKILL.md. This brief plus
`../BRIEF_FIX.md` is the complete contract. Spend your tool budget on the Rust source.

Worktree (already created, use it, do not create another):
`/home/zou/data/workspace/Aleph-wt-fix-w4b` (branch `fix-w4b`, base `main@777f37616`)

Contract: `/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/BRIEF_FIX.md`
(findings source of truth: same directory, `findings/B5.md`)

## Scope — exactly one finding
- **B5-001** [P1] `aleph-server stop` can SIGTERM/SIGKILL a recycled, unrelated PID
  (`src/bin/aleph-server/daemon.rs`, `handle_stop`).

Context you must read before changing anything: an earlier fix added a guard that refuses to stop a PID
that is *not* backed by the IPC endpoint file. That guard only covers the **PID-file fallback** path.
The dangerous path is still open: the endpoint file `.ipc-endpoint.json` is stale (server crashed or
rebooted), the OS recycled its PID for an unrelated process, and `endpoint_pid == Some(pid)` so the
guard passes and the unrelated process is signalled.

## Direction (verify it yourself, then implement the minimal correct fix)
- A PID alone is not proof of identity. Bind the endpoint record to something the live process still
  owns — e.g. the endpoint file's recorded start time / a boot-unique token / the endpoint's own
  liveness — and verify that binding before sending a signal.
- If the identity cannot be proven, do **not** signal: print a clear message that the recorded PID
  could not be verified as an Aleph server and exit non-zero (or report stale state) — never signal.
- Keep `stop` working for the normal case: a live server started by this user must still be stopped
  (both the endpoint path and the PID-file path).
- Prefer a fail-closed check over a heuristic such as process name matching.

## Output
- Conventional commit on branch `fix-w4b`, e.g. `fix(daemon): verify the recorded server identity before signalling a pid`.
- Report at `fixes/w4b.md` inside
  `/home/zou/data/workspace/Aleph-wt-fix-w4b/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w4b.md`:
  a `## B5-001 [P1] ...` section with `- verdict: fixed|partially-fixed|not-applicable|deferred`,
  `- files:`, `- change:`, `- risk:`.
- No cargo / clippy / rustfmt / tests (the orchestrator runs the unified `cargo check`).
- Final chat reply: max 12 lines — commit sha, files changed, how the identity is now proven, residual risk.
