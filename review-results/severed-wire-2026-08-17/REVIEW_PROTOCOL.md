# Severed-Wire Audit Protocol — 2026-08-17

## Mission

Static, **read-only** review of designated Rust modules for "severed wires":
symbols that are produced but never consumed, inert config knobs, dead
scaffolding, and name-drift. You are a reviewer, **not** a fixer.

## Working tree

- Read code from: `/home/zou/data/workspace/Aleph/.worktrees/review-fix-2026-08-17`
- Code graph context (may be **stale** — graph built at commit 9841b5b2, HEAD is
  newer; verify every claim with `rg`): `/home/zou/data/workspace/Aleph/graphify-out/GRAPH_REPORT.md`
  and `/home/zou/data/workspace/Aleph/graphify-out/graph.json`. Use sparingly —
  graph.json is huge (94K nodes); prefer `rg` parity for ground truth.
- Prior review reports exist under `review-results/` (e.g. `review-results/group_chat.md`,
  `review-results/mcp-batch-*`) — reference them as `existing_review_ref`, but
  **re-verify against current code**; findings there may already be fixed or stale.

## Method: PRODUCED–CONSUMED symbol parity

For each candidate symbol:

1. Identify the production definition/producer (struct / const / fn / field / variant).
2. Find ALL consumers repo-wide:
   `rg -n "<symbol>" src/ bin/ interfaces/ shared/`
   **Use `rg`, not bare `grep -n`** — this tree has CRLF checkout quirks where
   `grep -n` on a single file silently returns nothing (documented in
   `review-results/audit-cmd/seam.md`). A `grep -n` that returns empty is **not**
   evidence of absence.
3. Classify each consumer: production vs `#[cfg(test)]` vs dead code.
4. Decide: **CUT / CONNECT / DECIDE**.

Every "no consumer" claim MUST be backed by an explicit `rg` invocation (paste
the command + result in the report). A symbol referenced only by its own
definition plus its own test module is *test-only*, not production.

## Severed-wire forms

| Form | Meaning |
|------|---------|
| 1 | Visible symbol with zero production consumers (dead code) |
| 2 | Declared-but-never-wired stub/skeleton (module/struct/fn nothing calls) |
| 3 | Consumer references a symbol that no longer exists or was renamed (name-drift / stale references) |
| 4 | Produced but consumed only by tests |
| 5 | Name-drift residue: naming/constants/paths describing a reality that no longer exists (e.g. a const pointing at a config format the loader no longer reads) |
| 6 | Orphaned public API surface: pub items re-exported but unused, `#[allow(dead_code)]` items, `#[deprecated]` items past their stated delete date |

## Decisions

- **CUT** — delete. Safe when: zero production consumers, removal cannot break
  runtime behavior, no doc/contract promises the API. Include exact removal
  instructions (which lines/files).
- **CONNECT** — the symbol is inert but a live counterpart SHOULD consume it
  (e.g. a config field whose reader was dropped). Include the wire-up plan.
- **DECIDE** — needs human judgment (ambiguous intent, behavioral risk, config
  compat). Do NOT propose a concrete deletion; state the options.

## Severity

- **critical**: crash/panic reachable from untrusted input, data loss, security
- **high**: clear contract violation, silent behavior break
- **medium**: inert-but-meaningful surface (inert config knob, orphaned pub API)
- **low**: pure dead code, harmless leftover

## Report format

Write TWO files under `review-results/severed-wire-2026-08-17/<module>/` in the
working tree:

1. `REPORT.md` — human-readable findings with evidence: exact symbol,
   paths:lines, `rg` evidence, rationale, proposed change, risk, verification steps.
2. `summary.json` — machine-readable, schema:

```json
{
  "audit": "severed-wire-audit",
  "date": "2026-08-17",
  "module": "src/<module>",
  "files_scanned": ["..."],
  "method": "PRODUCED - CONSUMED symbol parity (rg across src/, bin/, interfaces/, shared/), read-before-write triage",
  "findings": [
    {
      "id": "sw-<mod>-<n>",
      "module": "src/<module>",
      "files": ["path:line", "path:line"],
      "severity": "critical|high|medium|low",
      "form": 1,
      "produced": "<symbol or API description>",
      "produced_location": "path:line",
      "consumer_location": "none found",
      "decision": "CUT",
      "rationale": "...",
      "proposed_change": "...",
      "risk": "...",
      "verification": "...",
      "existing_review_ref": null
    }
  ]
}
```

## Constraints

- **READ-ONLY.** Never modify/create/delete any file except your two report files.
- No `cargo check` / `cargo build` / `cargo test` runs.
- No style nits as findings (rustfmt/clippy noise is out of scope).
- Be pragmatic: a finding must be actionable and worth the diff. If unsure,
  prefer reporting as `low` + `DECIDE` over dropping it.
- Sanity-check every path:line at the end (the fixer will use them for surgical edits).
