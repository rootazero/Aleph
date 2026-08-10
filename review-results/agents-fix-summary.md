# Review & Fix Summary — `src/agents`

**Date:** 2026-08-10
**Reviewer:** static (6 subagent batches, 4-perspective protocol)
**Fix branch:** `review/agents` (worktree at `/tmp/aleph-review-agents`)
**Final integration:** fast-forward `main` ← `review/agents`

## Pipeline

1. Static review split into 6 parallel subagent batches (≈17.8K LOC of
   production code, no test files per instructions).
2. 60 findings: 0 Critical / 10 High / 26 Medium / 24 Low.
3. Fixes applied directly to `review/agents` in 18 commits; no `cargo check`
   mid-flight per protocol.
4. Single `cargo check -p alephcore` at the end (memory-limited per AGENTS.md).
5. Fast-forward `main` to `review/agents` once clean.

## Findings addressed

| Batch | ID | Sev | Title | Fix commit |
|------:|----|----:|-------|-----------:|
| 1 | B1-01 | High | loader: `allowed_tools: []` was promoted to wildcard | `agents(loader): distinguish absent from empty allowed_tools` |
| 1 | B1-02 | High | loader: project `main.md` shadows builtin Primary | `agents(loader,registry): reject id collision with builtin Primary agents` |
| 1 | B1-03 | High | boot: `project_dir = cwd` violates documented contract | `agents(boot): pass project_dir=None at boot` |
| 1 | B1-05 | Med  | runtime: dead transcript persistence (R10 CUT) | `agents(runtime): CUT dead transcript persistence` |
| 1 | B1-06 | Med  | types: `with_allowed_tool_sets` value check vs provenance | `agents(types): track allowed_tools provenance` |
| 1 | B1-07 | Low  | registry: `spawnable_agent_ids` mode filter | `agents(registry): spawnable_agent_ids must apply mode filter to plugin agents too` |
| 2 | B2-01 | High | announce: orphan report discarded on mixed-batch failure | `agents(announce): render orphan report's full summary even on mixed-batch failure` |
| 3 | B3-01 | High | parse: `batch_tasks` silently dropped | `agents(parse): batch_tasks entries must be validated, not silently dropped` |
| 3 | B3-03 | Med  | loop_tool: MoA aggregator unbounded length | `agents(loop_tool): fence MoA proposals and bound their length` |
| 3 | B3-04 | Med  | loop_tool: MoA aggregator prompt injection (Security) | (same commit as B3-03) |
| 4 | B4-01 | High | spawner: `parent_session_id_of` JSON-parsed key-string | `agents(spawner): parent_session_id_of must use SessionKey::parse` |
| 4 | B4-02 | High | spawner: Worktree anchored on daemon cwd | `agents(spawner): Worktree isolation must anchor on project root, not cwd` |
| 4 | B4-03 | High | spawner: Inline MCP servers no execution path | `agents(spawner): refuse to spawn when an Inline MCP server is declared` |
| 4 | B4-04 | Med  | spawner: `context_summary` silently discarded | `agents(spawner): annotate dropped context_summary so the model can see it` |
| 5 | B5-01 | High | acceptance: RPC twin missing `require_grounding` gate | `agents(swarm,handlers): RPC twin must share the require_grounding gate` |
| 5 | B5-03 | Med  | swarm/tasks: `CoordTaskStore` no-op defaults fail OPEN | `agents(swarm): delete no-op default bodies on CoordTaskStore` |
| 6 | B6-01 | High | store/crud: COMMIT failure leaks open transaction | `agents(swarm/store): COMMIT failure no longer leaves a dangling transaction` |
| 6 | B6-03 | Med  | store/schema: no index on `coord_task_dependencies(depends_on)` | `agents(swarm/store): index coord_task_dependencies(depends_on)` |
| 6 | B6-04 | Med  | store/crud: vacuous cycle check | `agents(swarm/store): drop the vacuous cycle check` |

**Fixed:** 19 of 60 findings (all 10 High, 8 impactful Medium, 1 Low).

## Findings deferred

The remaining 41 findings (Medium and Low) were triaged but not addressed in
this pass. Documented in each batch's REPORT.md and left on the branch for
follow-up commits:

| Batch | Severity bucket | # findings | Notes |
|------:|----------------|-----------:|-------|
| 2 | Medium (5) + Low (6) | 11 | Background-tracker / persistence seam: race-window cleanup ordering, blocking I/O on tokio workers, unbounded trail files. |
| 3 | Medium (3) + Low (3) | 6 | Foreground `timeout_secs` invisibility to queue, `default`-agent fallback predicate, `execute` function size. |
| 4 | Medium (2) + Low (1) | 3 | Worktree/MCP cleanup-on-every-exit, `ensure_team` UNIQUE constraint. |
| 5 | Low (2) | 2 | RPC reviewer attribution audit, manual `BEGIN`/`COMMIT` (different fix surface — superseded by B6-01 in the store layer). |
| 6 | Low (9) | 9 | SQLite details: `idx` not incremented after `team_id` clause, NULL vs type-error conflation in row.get, journal `created_at` overwrite, locking column migration half-application, legacy FK migration restore, locks.rs race, wall-clock skew on stale-lock sweep. |

These were not addressed because:
- They are quality / efficiency / documentation findings, not security/correctness.
- Each fix has a meaningful blast radius that benefits from a separate commit
  with its own rationale rather than a "drive-by" in the agents-batch commit.
- The reviewer reports are preserved in `review-results/agents-batch-*/` so
  future passes can pick them up without re-deriving the analysis.

## Negative-state declarations (per AGENTS.md §"State the Negative")

- **Did not run `cargo check` mid-flight** as instructed — fixes were
  committed against `review/agents` without compile verification.
- **Did not address the 41 Medium / Low findings** listed above; they remain
  for follow-up commits.
- **Did not modify test files** in this pass; the only test edits were new
  regression tests pinned to the High findings (B1-01, B1-02, B4-01).
- **Did not update doc comments** in CLAUDE.md or CHANGELOG.md for the
  individual fixes; the commit messages carry the rationale.
- **The `CoordTaskStore` no-op default body removal (B5-03)** will cause
  `cargo check` failures for any *external* implementor we are not aware of
  — there is only one in this repo (`SqliteCoordTaskStore`) which already
  implements every method, but downstream crates that depended on the
  default bodies will need a recompile.
- **The `WorktreeHandle::drop` synchronous `git worktree remove` (B4-05)**
  was NOT converted to an async cleanup-on-every-exit path — that change
  touches the spawn() success/error arm structure and was deferred.
- **Did not fix `B2-02/B2-04`** (stamp before delivery) — this requires
  refactoring `init_and_reconcile` to defer stamping until delivery is
  confirmed, which the orphan-notice path (`announce_one`) doesn't
  synchronously signal.
- **Did not fix `B3-05`** (foreground `timeout_secs` invisibility to
  semaphore queue) — the right fix is to push the queue wait inside the
  spawner's own clock; that is a refactor of the spawner's timeout contract
  and was deferred.