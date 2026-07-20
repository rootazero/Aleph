# Module: tasks

**Date**: 2026-07-19
**Reviewers**: 4 parallel agents (security × 2, logic, architecture)

## Summary
- Path: `src/tasks/` (~14k LOC across cron/, heartbeat/, mic_level/, presence/, shared/)
- Raw issues found: ~40
- After filtering (high-confidence only): 5

## High-Confidence Issues (will fix)

### 1. `PresenceReporter` defaults to `enabled: true` — HIGH (security/privacy)
- **File**: `src/tasks/presence/config.rs:20-22`
- **Description**: Default config broadcasts hostname + username (PII) every 30s on the Gateway event bus to every subscriber — including remote channels and shared log sinks. Privacy-violating by default; must be opt-in.
- **Fix**: Change `default_enabled()` to return `false`.

### 2. Carry-over filename collision via non-injective sanitisation — HIGH (data integrity)
- **File**: `src/tasks/cron/carryover.rs:104-127`
- **Description**: `sanitise_job_id` maps `/`, space, `+`, `@`, etc. all to `_`, so `foo/bar_baz`, `foo_bar_baz`, `foo bar baz`, `foo+bar/baz` all collide on `foo_bar_baz.json`. Two distinct jobs read/write each other's partial progress.
- **Fix**: Append a 32-bit FNV-1a hash of the raw id to the sanitised prefix → `{safe}-{hash}.json`.

### 3. Template double-substitutes `{{env:...}}` after splicing user data — HIGH (security)
- **File**: `src/tasks/cron/template.rs:80-84`
- **Description**: `ENV_RE.replace_all` runs LAST, scanning the whole result. A previous run's output (`last_output`) or a poisoned `context_vars` payload containing `{{env:AWS_SECRET_ACCESS_KEY}}` gets expanded against the live process environment, exfiltrating the value into the next prompt sent to the LLM.
- **Fix**: Move env-var substitution to the top of `render_template`, before `last_output` / `context_vars` are spliced in.

### 4. Carry-over cleared before execution, lost on failure — HIGH (data integrity)
- **File**: `src/tasks/cron/executor.rs:481-492`
- **Description**: `build_cron_prompt` reads the carry-over then immediately clears the file, *before* `adapter.execute` runs. Any execution failure (timeout, panic, permanent error) leaves no carry-over for the next firing — the partial progress is permanently lost.
- **Fix**: Remove the read-time `clear`. The post-run branch already handles both cases (writes a fresh partial on BudgetExhaustedPartialResult; clears idempotently when the run completed cleanly).

### 5. Stagger ceiling can wrap on extreme inputs — LOW (logic)
- **File**: `src/tasks/cron/stagger.rs:54-56`
- **Description**: `windows = lag / stagger_ms + 1` can overflow when `stagger_ms = 1` and `lag = i64::MAX`, producing `i64::MIN` after the `+1`. `saturating_mul` further down then underflows.
- **Fix**: Use `saturating_div(...).saturating_add(1)`.

## Skipped Issues (low signal / design choices / high risk)

- **R2/R4-style concerns in `executor.rs`** (cross-layer gateway imports, concrete `AgentRegistry`, business logic in metadata builder) — architectural, requires product owner sign-off.
- **P2 file-size violations** in `concurrency.rs` (1362 lines), `config.rs` (978 lines), `executor.rs` (828 lines) — refactor would touch every test.
- **Dead code**: `CronJob::to_delivery_payload` (config.rs:473) — kept private under `#[allow(dead_code)]` instead of removing (lower blast radius). `JobRun` struct (config.rs:588) is re-exported via `cron/mod.rs:51`; removing the public re-export would be a breaking API change. Deferred.
- **`executor.rs:174` timeout rounding** (1999 ms → 1 s) — minute granularity acceptable; existing semantics.
- **Isolated session task_id collision on same-ms overlap** — UUID `run_id` keeps events distinguishable even if `SessionKey` collides; acceptable.
- **`store.rs:66` `reload_if_changed` always returns true** — implementation already does the right thing on a write-coalescing path; cosmetic.
- **Webhook SSRF default policy uncertainty** — depends on `SsrfPolicy::default()` which is out of scope.
- **Webhook body size** — reqwest applies its own limits; per-endpoint cap is a config concern.
- **`chain.rs` coalescing of multiple pre-job triggers into one timestamp** — design choice for at-most-once semantics.
- **Carryover 30-day retention conflicting with monthly cadence** — retention policy is operator-configurable; not a code bug.
- **`spawn_periodic_carryover_sweeper` OnceLock init outside runtime** — race window only matters for non-Tokio callers; documented elsewhere.
- **Cron timezone ambiguity / DST** — cron-rs handles; the `tz` field on `ScheduleKind::Cron` is honored.
- **History / presence PII fields without `skip_serializing_if`** — needs per-channel redaction layer; broader design.

## Status
- 5 high-confidence issues fixed (1 HIGH privacy, 1 HIGH data integrity, 1 HIGH security, 1 HIGH data preservation, 1 LOW overflow).
- Committed without per-module `cargo check` per user instruction.
- Full project `cargo check` deferred to end of sweep.