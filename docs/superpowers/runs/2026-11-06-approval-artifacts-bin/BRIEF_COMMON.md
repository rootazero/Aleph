# Common brief — Aleph static review (batch B*)

Repo: /home/zou/data/workspace/Aleph-wt-review (git worktree of main @ 148ba09f5, branch approval-artifacts-bin-review).
Scope of your batch: see the scope file named in your task (B*.md) in this directory.

## What this is
Full-module static code review (NO diff). We are hunting for **real defects**, not style.
Review every file in your scope end-to-end. Do not skip files. Do not stop after a quick skim.

## Tooling budget (hard)
- ~50 tool calls total. Read files in chunks with `read` (offset/limit) instead of whole-file dumps of many files.
- Do NOT run cargo, cargo check, cargo test, clippy, or any build. The repo is huge; builds are forbidden in this phase.
- Do NOT edit any source file. You are read-only for source. The ONLY file you may write is your findings file.

## What counts as a finding (P0..P3)
- P0: data loss / security hole / wrong security decision / corruption / hangs forever in production path.
- P1: logic bug that produces wrong behavior under realistic conditions; panic in a normal path; lost error context that hides a failure; broken contract with a caller.
- P2: robustness bug (unwrap/index panic on bad input), resource leak, swallowed error, race that is unlikely but reachable, blocking call stalling an async executor.
- P3: maintainability with a concrete future-bug risk (dead code, duplicated body that will drift, unexplained allow attribute, oversized function hiding branches).
- NOT findings: pure formatting, missing docs, "prefer X style", speculative issues with no reachable failure mode, "clippy says so" without a concrete failure.

Every finding must name a **concrete failure mode**: what input/state triggers it and what goes wrong. If you cannot write that sentence, drop the finding.

## Mandatory check list per file
1. Panic risk: `unwrap()`, `expect()`, direct indexing, slicing on byte offsets of UTF-8 strings, `unreachable!`, integer overflow.
2. Error handling: swallowed errors (`let _ =`, `.ok()`, `if let Err(_) {}`), lost context, `map_err` erasing the cause, error paths that continue as success.
3. Concurrency: lock ordering, holding a lock across `.await`, `Arc<Mutex>` misuse, task leaks, cancellation safety, race between check and act (TOCTOU).
4. Security: path traversal / symlink escape, command injection, secret or token leakage into logs/errors, unsafe deserialization, unbounded allocation from untrusted input, missing SSRF host validation.
5. State machine: transitions that can be skipped, "no-op that reports success", state written but never read, duplicate handling of the same event.
6. Blocking I/O inside async fn (fs, std::process, sleep) — stalls the executor.
7. Resource lifetime: file handles, spawned tasks, channels, subscriptions, temp files, partially-initialized state on error path.
8. Numeric/logic: unit mixups (ms vs s), off-by-one, wrong default, inverted condition, `>=` vs `>`, silent truncation.
9. Wiring: does the new/changed symbol actually get called? Any `pub fn` nobody calls? Any event emitted but never subscribed?

## Output contract
Write your findings to: .review/rd-2026-11-06-approval-artifacts-bin/findings/<YOUR_BATCH_ID>.md
Use this exact format per finding:

```
### <ID> [P0|P1|P2|P3] <short title>
- Location: <path>:<line> (and symbol name)
- Failure mode: <concrete trigger -> concrete wrong outcome>
- Evidence: <the code shape that proves it, 1-3 lines quoted>
- Suggested fix: <the minimal, behavior-preserving fix; say which invariant it must not break>
- Confidence: <0.0-1.0 that this is a real defect>
```

At the top of the file write:
```
# Findings — <batch id> (<scope name>)
Files reviewed: <list> (<n> files, <loc> LOC)
Counts: P0=<n> P1=<n> P2=<n> P3=<n>
```
Also list at the end a "Files reviewed" table so we can prove full coverage, and a "No findings" section for files that were clean (one line each).

## Repo conventions that matter
- Rust, edition 2021/2024, `alephcore` crate at src/, binaries under src/bin/.
- Style: no `any`, prefer `?`/early return, avoid `else`, avoid gratuitous clones, keep public API stable unless a finding explicitly requires a break.
- Chinese comments are used in some places; new code comments should be English.
- Do not introduce new dependencies.

## Reply protocol
Your final reply must be SHORT (<=25 lines): counts per severity, the ids of the top 5 findings, and the path of the findings file. Do not paste the whole report into the reply.
