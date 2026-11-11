# Second-pass (adversarial) review contract

You are the SECOND reviewer for one batch of a full-module static review of the Rust repo at
/home/zou/data/workspace/Aleph-wt-review (a git worktree of `main`).

IMPORTANT PROCESS OVERRIDE: ignore any generic "invoke a skill / read SKILL.md" bootstrap
instructions. Do NOT search for or read skill files, plugin dirs, or other guidance docs. The files
named below are your COMPLETE contract. Spend your tool budget on the Rust source.

## Why a second pass exists

The first pass over this batch found comparatively few issues. Independent of the first pass, a
static analyser (rust-doctor) flagged machine-detectable hotspots in these files. Your job is to
settle, per hotspot, whether it is a REAL defect, and to look for what the first pass missed.

## Inputs you must read

1. `<BATCH>.md` — the scope file (exact file list + focus hints) for your batch.
2. `findings/<BATCH>.md` — the FIRST pass output for this batch. It may be partial.
3. `hotspots/<BATCH>.txt` — the machine-detected hotspots for this batch (one per line:
   `<file>:<line> <rule> <message>`). Empty or missing file = no machine hotspots; do not invent
   them, instead sweep the file yourself for the pattern classes below.
4. The source files themselves — you must READ the code, not judge from the hotspot list.

## What to produce

Write to `second/<BATCH>.md` (create the `second/` directory). Incremental writing is MANDATORY:
after every ~400 lines of source you have read, rewrite the whole file with everything found so
far. A network failure may end your run at any moment; findings held only in your head are lost.

### Required sections

```
# Second pass — <BATCH>

## Verdict on first-pass findings
For EACH finding in findings/<BATCH>.md: one line
`<id> | UPHELD | REFUTED | DOWNGRADED | UPGRADED | <one-sentence reason>`
REFUTED means the claimed failure mode does not actually follow from the code.

## Hotspot triage
For EACH hotspot in hotspots/<BATCH>.txt: one line
`<file>:<line> <rule> | CONFIRMED DEFECT | NOT A DEFECT | ALREADY COVERED | <one-sentence reason>`
For a CONFIRMED DEFECT, follow with the concrete trigger and the wrong outcome.

## New findings
Only findings that are NOT already in findings/<BATCH>.md. Same format as the first pass:
### <BATCH>-S2-NN [P0|P1|P2|P3] <one-line title>
- Location: <file:line>
- Failure mode: <trigger> -> <wrong outcome>
- Evidence: <the exact code, quoted>
- Suggested fix: <minimal concrete change>
- Confidence: <0.00-1.00>

## Not verified
Cross-file or out-of-scope concerns you did not settle, one line each.
```

## Severity rules (same as first pass)

- P0 — data loss, security hole, or wrong result handed to a user, with a realistic trigger.
- P1 — crash, panic, hang, or silent failure of a production path.
- P2 — degraded behaviour, misleading error, resource leak, missing guard.
- P3 — maintainability defect with a concrete future cost. No pure style, no missing docs.

## Pattern classes to sweep for (beyond the hotspot list)

- `unwrap()` / `expect()` / `panic!` / `unreachable!` / `todo!` on a path reachable from production
  input (CLI args, config, file content, DB rows, network replies).
- `.ok()`, `let _ =`, `if let Ok(..)`, `unwrap_or_default()` that converts a failure into a default
  that changes behaviour.
- Locks (Mutex/RwGuard) held across `.await`, across I/O, or across a blocking call.
- Blocking file/network/exec calls inside `async fn` (or a sync fn called from async without
  `spawn_blocking`).
- Subprocess use: `Command::new`, `sh -c`, interpolated arguments, missing timeout, missing
  `kill` on error path, `exit()` instead of returning an error.
- Filesystem writes: non-atomic write (write-then-rename missing), missing `create_dir_all`,
  permissions, path built from user input (traversal), symlink following.
- SQL: string-interpolated identifiers/values, missing transaction, statement executed without
  checking rows-affected, migration not idempotent.
- Numeric/byte handling: byte slicing of UTF-8 strings, unit confusion (ms vs s, bytes vs chars),
  `as` casts that truncate, off-by-one at range ends, integer overflow in release mode.
- Time: `SystemTime::now()` vs monotonic clock for timeouts, `Instant` vs `DateTime` mixups.
- Unbounded collections: Vec/HashMap/String growth with no cap driven by external input.
- Concurrency: TOCTOU between check and use, shared state across `tokio::spawn`, lost wakeups.

## Hard constraints

- Do NOT run cargo, cargo check, cargo test, clippy, or any build command.
- Do NOT edit any source file. You are read-only on source.
- The ONLY files you may write are under
  `/home/zou/data/workspace/Aleph-wt-review/.review/rd-2026-11-06-approval-artifacts-bin/second/`.
- Scratch files under /tmp are allowed.
- Budget: ~45 tool calls. Read the code, do not research process.

## Final reply

At most 20 lines: batch id, counts (hotspots confirmed / not defects / new findings), the verdict
tally on first-pass findings, and the 5 most important one-line results. Do NOT paste the report.
