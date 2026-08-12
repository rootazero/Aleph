# src/config + src/context — Fix Summary

## Workflow
- Audited both modules with the /severed-wire-audit workflow (5 phases: scan
  seams, enumerate, triage, fix, guard).
- Static review only (no `git diff` against an open PR) at the call of
  `/severed-wire-audit review指定模块`.
- Reviewed on a `review/config-context` worktree branched off `main`, then
  fast-forwarded `main` once the commits were clean.
- `cargo check -p alephcore --lib --no-default-features` (with
  `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=1` to keep the Rust build under
  the 16 GB ceiling) is the only compile gate that was run.

## Findings Triage

### H2 (HIGH) — `cached_config_schema` silent fallback → 🟢 FIXED
- File: `src/config/patcher.rs:31-39`
- The `unwrap_or_else` was producing `{"not": {}}` — a jsonschema accept-all
  sentinel — when `generate_config_schema()` failed to serialize. A failure
  here means the type itself is unsound: the schema is derived from the same
  `Config` struct the validator consumes, so a serialization failure is
  exactly the moment when proceeding with an accept-all sentinel would
  silently disable every config-edit validation.
- Fix: replaced with a `.unwrap_or_else(|e| panic!(...))` that names the
  type-system soundness failure in the panic message.
- **Status: fixed in commit `fb4f942a5`.**

### L4 / M3 (MEDIUM) — `Config::migrate_fetch` is unconnected → 🟡 RETRACTED
- Originally listed as "dead wiring" — `migrate_fetch` not called from
  `load.rs`.
- **Re-grep showed it IS called**: `src/config/load.rs:181` invokes
  `config.migrate_fetch();` after the `toml::from_str` round-trip and the
  security SSRF overlay. The wiring is correct; the L4/M3 finding was a
  false positive caused by my not searching the right glob.
- **Status: retracted. No code change.**

### H1 (HIGH) — `is_default_session` snapshot risk → 🟡 NOT A BUG
- Pure defensive note about `SessionConfig::default()` possibly becoming
  env-derived. Currently `SessionConfig::default()` is a fixed struct, so
  there is no drift today. No code change.

### H3 (DOC) — `parallel_tool_concurrency` doc claim vs consumer behavior → 🟡 NOT A BUG
- The docstring says `0`/`1` disables the parallel fast path. The actual
  consumer (`subagent_spawner/mod.rs:765`) treats `Some(0..=1)` as the
  disable sentinel. The producer and consumer agree; the doc is sharper
  than the code. No code change.

### H4 (LOW) — `AcpConfig::default_adapters` reads static at every `Config::default()` → 🟡 NOT A BUG
- Currently the preset list is a static const. If it ever becomes runtime-
  mutable, that becomes a real consistency risk. No code change today.

### M1 (LOW) — `summary_utils` re-export & `pub fn` both → 🟡 NOT CHANGED
- Re-export chain (compact::mod re-exports strip_analysis_block and
  IDENTIFIER_PRESERVATION) is a deliberate two-path exposure. The non-test
  callers reach the re-export, the in-module tests reach the source. Drift
  risk is small because the re-export is `pub use`, not a wrapper.

### M2 (MEDIUM) — `pressure.rs` `pub fn` over-exposed → 🟢 PARTIALLY FIXED
- Original claim: `detect_content_ratio`, `estimate_tokens_smart`,
  `chars_for_token_budget`, `chars_for_result_token_budget` had no external
  callers.
- **Re-grep showed all four ARE external callers** (tools/scoped,
  tools/result_store, tools/result_processing, builtin_tools/file_ops/read,
  extension/hooks/output_budget, etc.). The M2 finding was a false positive.
- However, the same audit **did** identify other `pub fn`s with no external
  callers. Those were tightened in commit `fb4f942a5`:
  - `src/config/types/tools.rs`:
    `UnifiedToolsConfig::{fs_allowed_roots, git_allowed_roots, is_screen_capture_enabled, screen_capture_config, is_search_tool_enabled, search_tool_config, enabled_mcp_servers}` → `pub(crate)`
  - `src/config/types/tools.rs`:
    `ToolServiceConfig::per_tool_durations` → `pub(crate)`
  - `src/config/patcher.rs`:
    `ConfigPatcher::record_mtime` → `pub(crate)` (only used inside the same file)
  - `src/context/budget/preflight.rs`:
    `PreflightPipeline::{with_cache_stability, with_min_pressure_ratio}` → `pub(crate)` (only used by `default_pipeline`)
  - `src/context/budget/preflight.rs`:
    `PreflightPipeline::empty` → `#[cfg(test)]` (only used in the empty-pipeline test)

### M3 (LOW) — `patcher.rs` 1498 lines, `validate.rs` 705 lines → 🟡 NOT CHANGED
- Larger files; the module-level documentation makes the entries clear.
  Splitting would risk breaking the `register_*_handlers` macro plumbing.
  No code change.

### L1/L2/L3 (LOW) — minor visibility / style → 🟡 NOT CHANGED
- Stylistic; out of scope for a severed-wire audit.

## What I did NOT do

- Did **not** delete any code. Every `pub fn` I downgraded to `pub(crate)`
  has its own justification comment in the source so the next reviewer can
  reverse the call if a new consumer appears.
- Did **not** run `cargo test`. `[severed-wire-audit]` skill rules: **diff
  review, no test re-run** (the user explicitly said "无需 cargo check,
  直接提交" and "全部模块 review 完成后统一 cargo check").
- Did **not** push until `cargo check -p alephcore --lib` was clean.
- Did **not** enable `clippy -D warnings`, because a pre-existing
  `manual_is_multiple_of` lint failure in `src/cluster/node_approval.rs`
  (unrelated to `src/config` / `src/context`) would have failed the gate.
- Did **not** open a PR. The user said "无需 PR", and the local main
  branch was fast-forwarded to `fb4f942a5`.

## Verification

```
$ git log --oneline -3
fb4f942a5 config+context: tighten dead-code visibility and harden cached schema failure
dcd2c678c review-results: src/config + src/context severed-wire audit
484dd8bec merge: sync origin/main (clarification multi-question + thread persistence + memory knob) into local main

$ CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=1 cargo check -p alephcore --lib --no-default-features
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 47s
```

## Edge cases / unhandled validation

- `(HIGH)` `cached_config_schema` panic now propagates a startup crash on
  the (very unlikely) path where `generate_config_schema()` fails to
  serialize. The original code silently disabled all validation. Both
  outcomes are bad: one is silent data loss, the other is a startup crash.
  The panic is the right choice because the schema is generated from the
  same `Config` it validates, so a failure means the type itself is broken
  — better to crash loudly than to accept any input. **The `process-loop`
  operator should monitor `aleph-server` startup for this panic.**
- `(MEDIUM)` `pub(crate)` is the conservative demotion choice. If a sibling
  workspace later needs the accessor, the next PR can promote one or two
  back to `pub`; the comment block names the rationale in every case.
- `(LOW)` `AcpConfig::default_adapters` is a smoke alarm for the case
  where `AcpAdapterEntry::all_presets()` becomes runtime-mutable. If
  anyone adds dynamic preset registration, this is the place to revisit.
