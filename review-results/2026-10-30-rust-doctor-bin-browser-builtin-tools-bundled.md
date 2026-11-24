# rust-doctor static review — `src/bin`, `src/browser`, `src/builtin_tools`, `src/bundled`

**Date**: 2026-10-29 → 2026-10-30
**Branch**: `main` (fast-forwarded from the isolated worktree branch `rust-doctor-batch2`)
**Worktree used for development**: `.worktrees/rust-doctor-batch2` (branch `rust-doctor-batch2`, based on `main` @ `5aad9fde3`)
**Base commit**: `5aad9fde3`
**Commits landed on `main`**: `a5badc334` (60 fixes), `e534d63f8` (compile repairs)

## Method

| Stage | Tool / actor | Output |
|---|---|---|
| Machine scan | `rust-doctor 0.9.0` (whole workspace, `cargo clippy --workspace --no-deps`) | 9,130 diagnostics, **1,866** inside the four modules; the high-signal subset was split per chunk into `machine-*.md` hints |
| Semantic map | `graphify-out/graph.json` (139,474 nodes / 288,770 edges) — per-module subgraph: files, cross-module in/out edges, hub symbols | `.review/rd-batch2/graph-map.md`, fed to every reviewer |
| Review | reviewer subagents, **no diff**: whole-file reading of all 402 files / ~248k LoC in the four modules | **180 findings** (16 P1 · 61 P2 · 103 P3) |
| Triage | **Jev** (`jev-latest`) — calibrated `noul` batches over every P1 and P2 candidate | fix-now / verify-then-fix / defer split |
| Fix | 8 `worker` subagents, file-disjoint briefs, in the isolated worktree | 60 fixes landed, 7 partial |
| Verification | one unified `cargo check -p alephcore --all-targets` (`CARGO_BUILD_JOBS=2`, `lld` linker) | 0 errors, 0 warnings |

Per-module finding counts:

| Module | Files | LoC | Findings | P1 | P2 | P3 |
|---|---:|---:|---:|---:|---:|---:|
| `src/bin` | 51 | 25.0k | 34 | 5 | 15 | 14 |
| `src/browser` | 66 | 71.0k | 55 | 6 | 22 | 27 |
| `src/builtin_tools` | 281 | 150.6k | 76 | 5 | 22 | 49 |
| `src/bundled` | 4 | 1.6k | 15 | 0 | 2 | 13 |
| **total** | **402** | **248.2k** | **180** | **16** | **61** | **103** |

The full finding list with each item's Jev score and routing decision is tracked in
[`2026-10-30-rust-doctor-4modules-findings.md`](2026-10-30-rust-doctor-4modules-findings.md).
The reviewers' raw JSON deliverables, the machine-scan hints, the chunk plans and the per-batch fix
reports live under `.review/rd-batch2/` (untracked working directory).

### Which steps used Jev

Jev was used for **classification, filtering and routing only** — never for writing code and never for
judging Rust semantics, which stayed with the reasoning models:

1. **Classification** of all 11 first-round P1 candidates into "fix now" (score ≥ 0.80) — the scores
   landed at 0.81–0.92, so all 11 were accepted.
2. **Filtering** of the 50 first-round P2 candidates against the same criterion — 29 fix-now, 16
   verify-then-fix, 5 deferred (`R04-1`, `R12-2`, `R02-3`, `R07-3`, `B41-2`).
3. **Filtering** of the 16 P1/P2 candidates surfaced by the second review round — 9 fix-now,
   3 verify-then-fix, 4 deferred (`R03-2`, `R14-2`, `R10-1`, `R07-1`).
4. **Routing** of the accepted findings into file-disjoint fix briefs, including the
   *verify-then-fix* band, so a worker had to prove the defect against the code before changing it.

Jev's discrimination is narrow (0.60–0.93 on the accepted band), so the coordinator re-read every
brief before dispatch and every fix report after it came back — a Jev score is a routing hint, not an
authorisation.

## What was fixed

### `src/bin` (CLI entry points)

- **Path traversal in `plugins uninstall`** — an unvalidated plugin name was joined onto the plugins
  directory and recursively deleted; malformed names and symlinked plugin paths are now refused.
- **Self-updater wrote its installer to a predictable temp path** and then executed it (symlink
  follow); the installer now lands in a private `0700` temp directory via `create_new`, mode `0600`.
- **`aleph-migrate --fix` could never pass verification** — `report.errors` was never decremented.
- **`--fix` laundered a corrupted transcript** by overwriting the stored hash with the hash of the
  current file; the mismatch is now reported instead of accepted.
- **`--remove-orphans` deleted attachment files** whose session manifest was missing or unparseable;
  it now warns and skips.
- **Attachment writes could escape the migration root** through a symlink when the target path did not
  exist; unsafe/out-of-root paths are rejected.
- **`--end-date` dropped the end day itself** (filter was `<= midnight`); `--include-attachments` could
  never be turned off; a NULL `created_at` aborted the whole export.
- **`daemon stop` resolved the PID file before the IPC endpoint**, so a stale PID file turned `stop`
  into a no-op that deleted a live server's discovery file; `stop` also signalled a recycled PID.
- **Shutdown unwrapped the process-wide markdown-skill mutex** — a poisoned lock panicked inside
  `start_server` and skipped the rest of the teardown; both registry locks now recover from poisoning.
- **Bundled-content extraction fell back to the world-writable `/tmp/.aleph`** when the Aleph home could
  not be resolved; startup now fails instead.
- **FTS5 re-index duplicated every transcript** — `INSERT OR REPLACE` is a plain insert on an FTS5
  table, so each re-index appended a second copy.

### `src/browser`

- **Post-navigation SSRF audit re-read the URL navigation itself wrote**, so it could not fail when the
  landed URL was unknown; the landed URL is now read independently (`Page.getFrameTree`) and the tab is
  closed when it cannot be established.
- **History navigation kept the previous (allowed) URL** in the tab table when the `frameNavigated`
  event was missed, so the read-time SSRF guard passed on a tab that had moved to a blocked origin.
- **`localStorage` restore wrote origin A's payload into the origin the page actually landed on** and
  booked it under A.
- **The PID-reuse guard parsed the wrong `/proc` field** (`vsize` instead of `starttime`), so the
  bootstrap-launched Chrome was never killed; the field index is corrected.
- **`sanitize_session_key` was lossy**, so two distinct profiles could share one browser data dir and
  one `Chrome --user-data-dir`; profile directories are now collision-free.
- **`browser_wait_for { time_ms: 60000 }` waited 5 s** while reporting "Delay 60000ms"; the fixed-delay
  wait now really waits the requested delay.
- **A failed `browser_select` deselected every option**, wiping the element's previous selection.
- **A failed click/fill/type left the effect probe installed** in the page (listeners plus the
  `data-aleph-probe` attribute).
- **A recording was handed to ffmpeg with `-y`**, silently replacing an existing file.
- **Windows `/flag` store flags survived the principal-profile copy**, letting a principal inherit the
  operator's browser store.
- **`playwright_cli` parsed `[Snapshot](` on any stdout line**, so echoed page content could redirect
  the local file the server opens.
- **A child frame that failed to attach/capture was dropped silently** and then escalated into a
  whole-page refusal; nested child-frame ids from a child renderer's node space were merged into the
  parent-space carrier.
- **DOM walks recursed unboundedly** over engine-controlled depth (>1024 now fails explicitly); the
  `arc_swap` guard was held across the SSRF await; the live control mutex was held across
  `bind_control`; a relaunch proceeded after the displaced engine refused to die.

### `src/builtin_tools`

- **`browser_exec`'s input-side secret gate only scanned top-level actions** — text nested in
  `repeat`/`if` was typed into the page unblocked, and `evaluate` scripts bypassed the gate entirely
  (`browser_evaluate` enforces it). Both are now refused before execution.
- **User-profile e-mail redaction had a 22-character bypass** — profile PII reached the model verbatim.
- **`code_check` reported `ok: true` when a timed-out check produced no parseable diagnostics**;
  incomplete checks are now reported as failures with their elapsed timeout.
- **`agent_create` silently dropped the TOML row for ids of 33–64 chars** while reporting success, and
  overwrote an existing `SOUL.md` / `AGENTS.md` without checking the filesystem.
- **`agent_update` applied the live `allowed_users` write before the TOML write** and then reported a
  bare error; **`agent_list` instantiated every lazily-registered agent** as a side effect of a read;
  **`agent_delete` reported "workspace archived"** even when the archive rename failed.
- **`note_schema` writes were an unconditional force-overwrite** when `expected_hash` was absent —
  bypassing the hash guard the module exists for — and had no content size cap.
- **`mcp_read_resource` returned unbounded MCP content** inline to the model; it is now capped and
  reports `truncated` / `total_bytes`.
- **`decompress` ignored the documented `read_backstop_tokens` ceiling** for LLM-supplied `max_tokens`.
- **`goal(action='set', gate_command="   ")` created a gate that could never fail**, while `update`
  treated blank as "clear"; a cross-session pause persisted an uncapped note into another session's row.
- **`process_journal` joined deserialized row fields onto the store path without validation**
  (traversal read, write and `remove_dir_all`); malformed rows are now ignored and logged.
- **`runtime_manage` install spawned unconditionally** — no dedupe against an in-flight install.
- **`tool_usage(forget_orphans)` reported N rows deleted** even when the sidecar store was missing or
  the delete failed; the count is now verified against the store.
- **`browser_qa` had no harness budget row**, so the harness killed it at 300 s although its own worst
  case is 540 s; the budget is now **derived** from the tool's own ceiling (`MAX_QA_WAIT_MS` + margin),
  so the two cannot drift. `browser_exec` likewise only checked its 600 s clock between steps.
- **`browser_qa`'s read-time SSRF check ran once**, before a wait that can run for minutes.
- **`bound_screenshot_png` decoded the whole capture before applying its caps** and returned the
  unbounded original when decoding failed.
- **The desktop run-boundary cleanup released the NEW run's session** instead of the ended run's, and
  `scroll` swallowed the coordinate validation error (scrolling at the real cursor).
- **`agent_identity` export did blocking `std::fs` plus full-chain serialization inside an async tool
  call**; `bash_exec` misattributed the fleet-wide 32-job cap to the per-session 8-job cap.

### `src/bundled`

- **Filesystem-source skill copy wrote through nested destination symlinks** (symlink escape out of
  `~/.aleph/skills`) — the pilot reviewer proved it with a compiled replica of the copy helper; the
  sync now fails rather than writing through a planted symlink.
- **Prune removal failures were silently discarded** while the skill was reported as successfully
  synced; **pruning only covered the top level**, so nested stale official files survived forever.
- **One failing skill permanently blocked the `bundled_version` bump**, so every startup re-extracted
  and re-pruned all official skills.
- **A reconcile failure before extraction was only warned about**, so a user skill could be overwritten
  and pruned by the bundled skill of the same name.
- **The Windows rename fallback deleted the manifest and could leave it deleted**, losing all skill
  provenance.

## Verification

One unified check after all fixes, run on `main` with the shared (warm) target directory:

```
CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9m 36s
```

**0 errors, 0 warnings.** The first run of that check found 4 compile errors and 2 dead-code warnings
introduced by the fix workers (`res.result.value` against an `EvalResult` that has no `result` field,
a non-`#[non_exhaustive]` construction of `image::Limits`, a missing match-arm comma, and two
now-unused items). Those were repaired in `e534d63f8`; the second run was clean.

No test suite was executed: the brief asked for one unified `cargo check` at the end, and `cargo test`
for `alephcore` takes ~14 minutes on this machine.

## Not done (explicit negatives)

- **No tests were added.** The brief's fix rules allowed a regression test only where the module
  already had a natural test home; of the 60 fixes, none got a new test, so every fix is verified by
  review and compilation only.
- **No test suite was run** (`cargo test` was out of scope for this pass) — the fixes are compile-clean
  but not behaviour-verified by the suite.
- **7 findings were left partial**, each because a correct fix needs a file owned by another batch or a
  design decision that exceeds a static-review fix: `R06-3` (ChromiumEngine ignores the configured
  Playwright CLI pin), `R13-1`/`R13-2`/`R14-1` (live control mutex held across awaits, same-binding
  publish not revoking issued grants, backend-wide controls mutex held across `bind_control`),
  `R04-2` (the `tools.invoke` allowlist gate and `tools.effective` surface are built from a different
  agent catalogue than the agent loop uses), `R01-9` (legacy PID-file-only servers are no longer
  signalled — partially mitigated).
- **11 findings were deferred** by the Jev triage (9 listed above plus `R14-2` and `R10-1`): they are
  real but need either a design decision (a note-schema size cap, smart-recall expansion on the
  default search path) or are cosmetic (a wrong budget-cap message).
- **The 103 P3 findings were not scored by Jev and were not fixed** — they are recorded in the findings
  index for a later pass.
- **No `cargo fmt` / `cargo clippy` run.** Formatting of the fixed files was left to the workers'
  style-matching; `cargo check` does not enforce it.
- **Nothing was pushed.** Both commits are local on `main`.
- **`src/tools/budget.rs` and `src/gateway/agent_instance.rs` were touched although they live outside
  the four reviewed modules** — the first is the shared harness budget table the `browser_qa` fix
  needed, the second an additive accessor the `agent_list` fix needed.
