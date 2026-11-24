# Common Brief — Aleph Static Review (Batches B1–B14)

Repo: `/home/zou/data/workspace/Aleph-wt-browser-builtin-bundled` (git worktree of main @ 8e8ab3cd7, branch review-browser-builtin-bundled).
Scope of your batch: see `docs/superpowers/runs/2026-11-06-browser-builtin-bundled-review/B*.md`.

## What this is
Full-module static code review (NO diff) guided by `/simplifying-rust-with-occams-razor`. We are hunting for **real defects and unnecessary complexity**, not style preferences:
1. Occam's Razor: removal of unnecessary concepts, redundant abstractions, uncalled helpers, dead code.
2. Panic risks: `unwrap()`, `expect()`, direct indexing `vec[i]`, string byte slicing `&str[..]` on UTF-8, integer overflow.
3. Concurrency & Async: blocking I/O inside async fn (`std::fs`, `std::process`, `std::thread::sleep` on tokio runtime without spawn_blocking), lock holding across `.await`, cancellation safety, TOCTOU.
4. Error semantics: lost error context, swallowed errors (`let _ =`, `.ok()`), fail-open on security checks.
5. Invariants & Contracts: public API stability, Send/Sync, serialization formats, resource cleanup.

## Tooling budget (hard)
- Read files carefully using `read_file`.
- Do NOT run cargo, cargo check, cargo test, clippy, or any build commands. Builds are forbidden in this phase.
- Do NOT edit any source files during the review phase. Only write findings to your findings file.

## What counts as a finding (P0..P3)
- P0: Data loss / security bypass / remote code execution / deadlock / memory corruption.
- P1: Logic bug causing incorrect observable behavior; unhandled panic on reachable production input; blocking call stalling the Tokio async runtime thread; broken protocol contract.
- P2: Robustness defect (index/slice panic on malformed input), resource leak, swallowed error hiding real failure, TOCTOU race.
- P3: Maintainability / Occam's Razor debt (dead code, duplicate logic that can drift, unreasoned allow attributes, gratuitous allocations/clones on hot paths).

Every finding MUST include a **concrete failure mode**: what input/state triggers it, and what goes wrong.

## Output contract
Write your findings to: `docs/superpowers/runs/2026-11-06-browser-builtin-bundled-review/findings/<BATCH_ID>.md`

Use this exact structure per finding:
```markdown
### <ID> [P0|P1|P2|P3] <short title>
- Location: <path>:<line> (`symbol_name`)
- Failure mode: <concrete trigger -> concrete wrong outcome>
- Evidence: <the code shape that proves it, 1-3 lines quoted>
- Suggested fix: <the minimal, behavior-preserving fix; state invariants preserved>
- Confidence: <0.0-1.0>
```

At the top of the file write:
```markdown
# Findings — <BATCH_ID> (<Description>)
Files reviewed: <count> (<LOC> LOC)
Counts: P0=<n> P1=<n> P2=<n> P3=<n>
```

And at the end, list files reviewed with a clean status if no findings were found in those files.
