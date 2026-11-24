# rust-doctor static review — `src/config`, `src/context`, `src/diagnostics`

**Date**: 2026-10-30
**Base commit**: `569128f74` (main)
**Worktree used for development**: `.worktrees/rd-modules` (branch `review/modules-cfg-ctx-diag`)
**Scope**: whole-file static review, **no diff** — every `.rs` file of the three modules was read

## Method

| Stage | Tool / actor | Output |
|---|---|---|
| Machine scan | `rust-doctor 0.9.0` (whole workspace; `--rule rust_doctor::repo::hardcoded_credential=off --advisory-db /dev/null` — the credential rule panics on this repo's Chinese source files) | 9,158 diagnostics workspace-wide, **286** inside the three modules; 255 structural (duplicate bodies, oversized units, complex functions), 31 potentially behavioural (indexing, UTF-8 slicing, `expect`, `unreachable!`, blocking calls in async) |
| Semantic map | `graphify-out/graph.json` (139,519 nodes) — `graphify query` for the cross-module wiring of the three modules | fed to the reviewer briefs as an optional lookup |
| Review | **24 reviewer subagents**, whole-file reading of all 156 files / 65,754 LoC, split into file-disjoint briefs | **102 findings** (52 P1 · 42 P2 · 8 P3) |
| Triage | **Jev** (`jev-latest`) — one calibrated `noul` per finding | 59 fix-now · 30 verify-then-fix · 13 defer |
| Fix | **13 worker subagents**, file-disjoint briefs, in the isolated worktree | 79 fixed · 10 partial · 5 needs-other-file · 5 deferred · 1 rejected |
| Verification | one unified `cargo check -p alephcore --all-targets` (`CARGO_BUILD_JOBS=2`, `lld` linker) | **0 errors, 0 warnings** (9m46s) |

Per-module finding counts:

| Module | Files | LoC | Findings | P1 | P2 | P3 |
|---|---:|---:|---:|---:|---:|---:|
| `src/config` | 88 | 30,192 | 52 | 21 | 27 | 4 |
| `src/context` | 41 | 24,366 | 27 | 13 | 10 | 4 |
| `src/diagnostics` | 27 | 11,040 | 23 | 18 | 5 | 0 |
| **total** | **156** | **65,754** | **102** | **52** | **42** | **8** |

The full finding list with each item's Jev score, triage disposition and implemented outcome is in
[`2026-10-30-rust-doctor-config-context-diagnostics-findings.md`](2026-10-30-rust-doctor-config-context-diagnostics-findings.md).
The reviewers' raw reports live under `.review/rd-2026-10-30-modules/findings-B*.md` (untracked working directory).

## Which steps used Jev

Jev was used for **classification, filtering and routing only** — never for writing code and never
for judging Rust semantics, which stayed with the reasoning models
(`minimax-cn/MiniMax-M3`, `88api/gpt-6.1-sol`, `modelsell/gpt-6.1-sol`):

1. **Classification** — one `noul` score per finding (102 findings, one question each): "is this a
   real defect in the current code that should be fixed now with a contained safe fix?". The scores
   landed at 0.33–0.94.
2. **Filtering** — the score bands produced the three dispositions: `fix-now` ≥ 0.85 (59 findings),
   `verify-then-fix` 0.70–0.85 (30), `defer` < 0.70 (13).
3. **Routing** — each disposition became the instruction inside the file-disjoint worker briefs: a
   `fix-now` brief carried the concrete patch sketch, a `verify-then-fix` brief carried the
   verification step ("confirm the failure mode still exists; reject it with evidence if not"), a
   `defer` finding was excluded from the briefs and recorded here instead.
4. **Batch routing** — the 156 files / 65,754 LoC were split into 24 reviewer briefs by size and by
   module boundary (config write path / config read-write core / agent manager / provider types /
   generation types / policies / context fabric / budget / compactor / diagnostics framework /
   diagnostics checks), so no reviewer had to hold more than ~3.2k lines.
5. **Fix routing** — the 89 implementable findings were split again into 13 file-disjoint worker
   briefs, so two workers never edited the same file.

Jev's discrimination is narrow (0.33–0.94 on a 102-item spread), so the coordinator re-read every
brief before dispatch and **every diff after it came back** — a Jev score is a routing hint, not a
verdict. Twelve compile errors produced by the workers (none of which may run `cargo`) were found and
fixed by the coordinator in the final unified check.

## What was fixed

### `src/config` — the configuration write path and its value types

- **Atomic, permission-safe, section-preserving saves** (`save.rs`): the fixed-name `.tmp` staging
  file is replaced by the shared `utils::atomic_io` helpers (`write_atomic` + `with_file_lock`), the
  secret-bearing config is no longer written world-readable before `chmod 0600`, and a full save no
  longer drops externally owned sections such as `[gateway]` or the nested `[security.ssrf]` table
  the loader actually reads. Incremental saves now hold the advisory lock across read → merge →
  mtime recheck → replacement, and a section whose parent is not a table is reported instead of
  silently skipped.
- **Patch commit** (`patcher.rs`): the hot-application of a rollback is now reported as incomplete
  when a live target did not accept the restored config, instead of claiming success; the candidate
  normalization no longer rewrites an explicitly supplied `general.default_provider`.
- **Agent manager** (`agent_manager/*`): agent creation no longer drops `allowed_users` and other
  supplied fields, patching an inline `identity = { … }` table no longer erases the unpatched
  fields, concurrent agent mutations are serialized by a transaction lock, and a failed create or
  delete no longer leaves a half-applied definition behind. The whole config (including provider
  API keys) is no longer staged through a fixed-name world-readable temp file.
- **Effective config**: the ACP preset back-fill no longer promotes an explicitly disabled trust
  level back to `Full`, a custom-named ACP entry inherits its declared preset, the synthesized local
  voice provider is refreshed when `[voice.local]` changes, voice bearer tokens are no longer
  exposed through a derived `Debug`, and the spend ceilings reject `NaN`/`Inf`.
- **Honest configuration**: the tool-permission merge keeps the intersection of two glob
  allowance layers, the generation preset lookup no longer hands one vendor's endpoint to any
  provider of a protocol-level type, a default provider that cannot serve its modality is
  rejected, preset overrides now cover speech and transcription, and a stop-hook `timeout_secs = 0`
  can no longer silently disable the gate.

### `src/context` — the context fabric, the budget and the compactor

- **Admission is fail-closed** (`fabric.rs`, `fabric/tool_admission.rs`): an invalid host session
  identity no longer degrades into "no admission scope", the claimed input/output size is measured
  against the payload instead of trusted from the wire, and a payload carrying credential material
  is refused.
- **Budget accounting** (`budget/`): a non-positive or non-finite `token_estimate_ratio` is
  rejected, the external preflight now measures a staged candidate with the budget's own
  estimator (the ratio travels with the pipeline), and the file-operation key normalises `.`/`..`
  so two spellings of one path share one key.
- **Cheap passes** (`budget/cheap_passes/`): a partial edit no longer supersedes the entire earlier
  file read, two different paths no longer share one key, and image stripping reports net savings.
- **Compaction** (`compact/`): the "already compacted" guard probes the shape the code actually
  produces, the extend-merge arm checks cancellation before committing its splice, a failed child
  seed no longer closes the parent first, split summaries no longer drop tool failures, a cleared
  scratchpad no longer resurrects an older plan, an unfinished plan survives recompaction, marker
  text in an ordinary message can no longer impersonate a plan carry, and reused run ids no longer
  move the compaction cut across closed runs.

### `src/diagnostics` — the doctor must be honest and non-destructive

- **Redaction** (`redact.rs`): URL userinfo credentials, non-Bearer/Basic authorization schemes,
  JSON secret fields, quoted assignment values, multiline PEM private keys and query tokens
  starting with `*`/`=` are now masked — each with a concrete example string in the finding.
- **Non-destructive checks**: the config-parse check no longer persists a config migration during a
  read-only diagnostic and no longer echoes the offending TOML line (which can carry a real API
  key); the data-dir write probe uses a unique `create_new` file and removes only what it created
  (it used to truncate an existing file and follow symlinks); the duplicate-instance check no
  longer reports the single healthy daemon as a duplicate when the doctor itself is the scanned
  process; the disk-space check uses the capacity available to an unprivileged process; the Discord
  probe parses TOML like the loader does; the session-log check reports an unreadable projection
  instead of a clean result and bounds its memory use; fold economics handle negative and
  overflowing token counts and report undecodable events; a pinned engine file must be executable;
  a probe failure is never reported as "healthy".
- **Report integrity** (`check.rs`, `mod.rs`): a panic in one check is now contained at the
  execution boundary and reported as a finding, so it cannot abort the whole report.

## Deferred (needs a decision or a change outside the reviewed modules)

| id | What | Why it was not fixed now |
|---|---|---|
| B08-8 | The preset catalogue bypasses the merge function, so a `presets.toml` override is defeated | The consumer is `src/gateway/handlers/generation_providers/handlers.rs`, outside the three reviewed modules |
| B09-1 | Configured embedding dimensions are silently bypassed by response truncation | The truncation happens in the provider consumer, outside `src/config` |
| B17-1 | The session split commits the parent close before the child seed (partial) | Making it atomic needs the event-store transaction boundary, outside `src/context` |
| B15-1, B15-4, B19-2, B19-3, B19-5 | Telemetry under-reporting, a stale flow doc, plugin ownership, per-plugin timeouts, a visibility doc | Jev < 0.70 — no contained safe fix, or the fix is a design decision |
| B05-1, B07-3 | Model-compat knobs and four routing controls without a runtime consumer | Jev < 0.70 — removing or wiring them is a product decision |
| B05-2 | Provider config accepts misspelled keys (rejected) | The repo already has a dead-key warning mechanism; adding `deny_unknown_fields` would reject legitimate configs. A regression test was added instead |

## Residual risk (stated explicitly)

- `save_to_file` now re-adds unknown top-level sections that exist in the file on disk. A user who
  *deletes* a section by hand and then saves from the app will see it come back. This is the price
  of not destroying sections the loader accepts but `Config` does not model.
- The advisory lock in `save.rs` only serialises the participants that take it. Other writers of
  `config.toml` (the agent manager's own writer, for example) still need to take the same lock;
  that was not part of this pass.
- The credential check in `src/context/fabric.rs` detects credential *shapes*. It cannot prove the
  provenance of arbitrary content, so a secret in an unfamiliar shape can still pass.
- Reviewers read the files as they are; no test was executed against the changes. The unified
  `cargo check` proves the tree compiles, not that the new behaviour is correct at runtime.

## Test run

No test suite was executed: the pass was scoped to static review plus one unified compile check,
and the machine's memory budget (19 GB) does not allow a `cargo test -p alephcore --lib` build
alongside the review fleet. The tests added by the workers compile (`--all-targets` includes them);
they have not been run.
