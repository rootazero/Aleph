# BRIEF_COMMON — house contract for the 2026-11-09 occams review (extension / fetch / gateway)

## What this round is

A **static** review (Occam's Razor skill) of three modules, no diff involved: `src/extension`, `src/fetch`, `src/gateway`
(822 files / 413,548 LOC). Findings are fixed on `main` afterwards, in commits, **no PR**.

Two model tiers are used (the user asked for a cheap tier doing classification/triage/routing and a
reasoning tier doing the deep analysis):

| Tier | Model | Job |
|---|---|---|
| Cheap ("Jev" role) | `minimax-cn/MiniMax-M3.1-Flash` | candidate sweep: read the batch, list *suspects* with file:line + one-line symptom. **No severity, no fix, no verdict.** |
| Reasoning | `minimax-cn/MiniMax-M3` | deep review: verify each candidate against the code, review the batch independently, assign P0–P3, propose the minimal fix |
| Adjudication | `88apiclaude/claude-sonnet-5-5` + `88apigpt/gpt-6.1-sol` (raw magpie API) | try to **refute** each finding; findings that do not survive are downgraded/dropped *with the reason recorded* |

There is no model literally named "Jev" on this machine (magpie serves: `minimax-cn/*`, `88apigpt/*`,
`88apiclaude/*`, `kimi/*`, `codex/*`). The "Jev" role is therefore executed by the **cheapest available
model** (`minimax-cn/MiniMax-M3.1-Flash`), by role, not by name. This substitution is stated in the
round README.

## Hard rules (violating any of these invalidates the finding)

1. **Read every file in your batch in full.** No sampling, no `head`, no skim. A file you did not read
   cannot produce a finding, and must not appear in your "clean files" list.
2. **Graphify first.** Before opening files, run `graphify query "<your question about this area>"`
   (in `/home/zou/data/workspace/Aleph`; graph lives in `graphify-out/`). It is orientation, not proof.
   The graphify hook blocks raw source reads until you have run a graphify query in your session.
3. **No cargo.** Do not run `cargo check` / `test` / `clippy` / `build` / `rustc`. This machine has 20 GB
   RAM and no swap; a single `alephcore` lib-test compile peaks at ~16–18 GiB RSS. A unified
   `cargo check` happens once at the end of the whole round.
4. **Do not edit source files.** The review phase only writes findings files under the round directory.
   (The separate fix phase edits code.)
5. **No git mutation.** No `stash`, `checkout --`, `restore`, `clean`, `commit`, `push`, `rebase`.
   `git status`, `git log`, `git ls-files`, `git show` are fine.
6. **Do not touch files outside your batch.** Other agents own the other batches; a cross-batch edit
   destroys the round's file-disjointness.
7. **Every finding needs a concrete failure mode.** "Trigger → wrong outcome", with real inputs or a
   real code path. A finding without one is a style opinion — drop it.

## The five review dimensions

1. **Occam's Razor** — unnecessary concepts: duplicated state, redundant abstractions, needless
   indirection, speculative generality, dead code (only with evidence: no reference found ≠ dead code —
   check tests, examples, bins, `cfg`/feature gates, serde names, FFI, plugin hooks, generated code).
2. **Panic risks** — `unwrap`/`expect`/`panic!`/direct `vec[i]`/integer overflow/UTF-8 byte slicing on
   a path reachable from a public entry point, hostile input (network, files, plugin manifests), or a
   poisoned state.
3. **Concurrency & async** — blocking I/O inside `async fn`, lock held across `.await`, lock-order
   inversion, poisoned locks, cancellation safety, TOCTOU, unbounded channels/semaphores, spawn
   without join/ownership.
4. **Error semantics** — swallowed errors, lost context, `unwrap_or_default()` on a security-relevant
   check, fail-open where the contract says fail-closed, error variants changed, errors that lose the
   user's ability to act.
5. **Invariants & contracts** — public API/signature changes, error-variant changes, `Send`/`Sync`
   regressions, ordering / idempotency / retry changes, serialization & wire-format changes, resource
   leaks.

## Severity

| P | Meaning |
|---|---|
| **P0** | data loss, security bypass / secret leak, RCE, deadlock / livelock, memory corruption, identity confusion |
| **P1** | real logic bug, unhandled panic on a reachable path, blocking call stalling the runtime, broken protocol, swallowed error that changes decisions |
| **P2** | robustness defect, resource leak, TOCTOU, missing bound, unbounded allocation, redundant work on a hot path |
| **P3** | maintainability / Occam debt with no behavioral consequence (still must have a named consequence) |

## Finding template (exact)

```
### <ID> [P0|P1|P2|P3] <short title>

- **Location**: `<path>:<line>` (`<symbol>`)
- **Failure mode**: <trigger> → <wrong outcome>
- **Evidence**: <1–3 quoted lines from the file>
- **Suggested fix**: <minimal, behavior-preserving; state which invariants are preserved>
- **Confidence**: <0.0–1.0>
```

`<ID>` = batch id + sequence, e.g. `G1-07`.

## Output files

- Cheap tier → `docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/candidates/<BATCH_ID>.md`
- Reasoning tier → `docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/<BATCH_ID>.md`
- Findings file header: `# Findings — <BATCH_ID> (<title>)`, then `Files reviewed: <n> (<LOC> LOC)`,
  then `Counts: P0= n P1= n P2= n P3= n`.
- End the findings file with `## Clean files` listing every reviewed file with **no** finding.
- Adjudication verdicts are appended to the findings file as `## Adjudication` (append-only).

## Anti-false-positive rules (from the previous round — the reason the previous round's 86 findings
survived scrutiny)

- "No reference found" does **not** prove dead code.
- `clone()` is not a finding by itself; only on a proven hot path.
- "Missing abstraction" is not a finding.
- "Lock across await" only if the guard is provably held across an `.await`.
- "No bound" only if the value is attacker- or user-controlled and can actually grow.
- When in doubt, prefer **no finding** over a speculative one.
- Per-platform channel adapters look duplicated on purpose; report **specific divergences**, not the
  duplication.
