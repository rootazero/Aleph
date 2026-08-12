# Module: src/context

## Summary
- Path: `src/context/` (~21 files, ~11,942 lines)
- Three sub-areas: `budget` (pressure + preflight + cheap passes), `compact` (compactor + manual + session_split + rescue + summary_utils + tool_aware_chunker), `retrieval` (ContentIndex).
- Issues found: 3 medium, 4 low — see "Findings"

## Reviewers
- Wiring severed-wire audit (PRODUCED − CONSUMED)
- Static analysis (unwrap/expect/panic in non-test code)
- Configuration R1-R10 cross-check
- graphify-coupled entry/exit survey

## Severed-Wire Audit (Phase 1–3)

### Items verified as WIRED

- `compact::compactor::{ContextCompactor, CompactStrategy, CompactResult, CompactorConfig}` — used by `agents/subagent_spawner/mod.rs`, `bin/.../orchestrator_init.rs`, `builtin_tools/sessions/compact_tool.rs`, `memory/session_compactor/summary_source.rs`.
- `compact::manual::{ManualCompactOptions, ManualCompactOutcome, ManualCompactWiring, install_manual_compaction, manual_summarizer, manual_keep_tokens}` — wired in `bin/.../orchestrator_init.rs:300`, `builtin_tools/sessions/compact_tool.rs:78/124/141`, `gateway/handlers/session/db_handlers/modify.rs:609`.
- `compact::session_split::perform_session_split` — wired in `context/compact/directive.rs:149`, `context/compact/compactor.rs:787`, `harness/tests/task10_wiring/extras.rs:75/161`.
- `compact::rescue::{drain_context_overflow, try_reactive_compact_and_retry, RescueCx, RescueHost, MAX_REACTIVE_COMPACT_ATTEMPTS}` — wired in `harness/agent/think.rs:590/606/649`, `tests/reactive_compaction.rs`, `tests/budget.rs`.
- `compact::tool_aware_chunker::{SemanticUnit, SemanticChunk, parse_semantic_units, ToolAwareChunker}` — wired in `memory/session_compactor/{chunker_tests, post_turn_compress}.rs`, `context/compact/compactor.rs` (chunker-tied).
- `compact::summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION, build_window_summary_prompt, prepend_user_instructions, build_summary_update_prompt}` — re-exported at `src/context/compact/mod.rs:32` so consumers reach them via `crate::context::compact::strip_analysis_block`. Used by `session_split`, `compactor`, `manual`, `memory/session_compactor`.
- `compact::directive::{DirectiveOutcome, ...}` — `harness/agent/think.rs:475/483` consumes the `SplitTo`/`FellThrough` variants.
- `budget::{ContextPressure, ContextBudgetConfig, ContextBudget, LoopDirective, ContextBudget.before_turn, note_compaction_effect, observe_actual_usage, seed_calibration, peek_pressure}` — wired in `orchestrator/harness_bridge/runner_impl.rs:1366/1416/1422`, `harness/tests/budget.rs:315`.
- `budget::preflight::{PreflightPipeline, default_pipeline, PreflightStage, ...}` — wired in `tests/preflight_cheap_passes_e2e.rs` and built by `builder/agent_init` (cf. `ts10_wiring`).
- `budget::pressure::{estimate_tokens_smart, content_ratio_with_baseline, detect_content_ratio, estimate_tokens_aware, estimate_message_tokens_aware, chars_for_token_budget, chars_for_result_token_budget}` — all consumed by `super::mod.rs` itself (the budget module is the primary consumer of its own helpers). Cross-check: `estimate_message_tokens_aware` is the only one exposed externally (used by `ContextBudget::compute`).
- `budget::cheap_passes::{FileOpSupersedeStage, ImageStrippingStage, ToolResultPruningStage, ...}` — wired into `preflight::default_pipeline` (the `cheap_passes` re-export from `budget/mod.rs`).
- `retrieval::{ContentIndex, IndexOutcome, SearchHit, IndexError}` — wired in `tools/result_store.rs:38/124/155/309/314` and `session/store.rs:253`.

### Findings (medium)

#### M1. `context::compact::summary_utils::strip_analysis_block` — exported twice
- Re-exported at `src/context/compact/mod.rs:32` and `pub` at `src/context/compact/summary_utils.rs:103`.
- Consumers reach `crate::context::compact::strip_analysis_block` (the re-export) — searches confirm no consumer reaches the `summary_utils` path directly. Same for `IDENTIFIER_PRESERVATION`.
- Severity: **LOW** (drift risk, not a bug).

#### M2. `context::budget::pressure::{detect_content_ratio, estimate_tokens_smart, chars_for_token_budget, chars_for_result_token_budget}` are `pub fn` but only used within `pressure.rs` itself
- Verified by `grep -rn "detect_content_ratio\|estimate_tokens_smart\|chars_for_token_budget\|chars_for_result_token_budget" --include="*.rs" src/` — only the `pressure.rs` self-references show up.
- Risk: dead public API. If they are intended internal-only, they should be `pub(crate)` or `pub(super)`. If they are intended external, they should be exercised by something.
- Severity: **MEDIUM** — silent contract expansion. Reducing visibility is the safer move.

#### M3. `context::compact::compactor.rs:25-45` — `CompactStrategy` and `CompactResult` are `pub` and `Clone`/`Debug` derived, but never returned by name from primary callers
- All callers get `Result<CompactResult, ...>` or `Result<Option<CompactResult>, ...>`. The `CompactStrategy` enum is used in `summary_source.rs:122` to mark `SessionMemoryReuse`. Pattern is fine.
- Severity: **LOW** (no action).

### Findings (low)

#### L1. `mod.rs` files are mostly re-exports
- `src/context/budget/mod.rs` re-exports `crate::providers::message::UnifiedMessage` for use by `pressure.rs`. The `// !` doc is short and accurate.
- `src/context/compact/mod.rs` re-exports `summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION}` and `tool_aware_chunker::*`. Pattern is consistent with `src/context/mod.rs`.
- `src/context/retrieval/mod.rs` is tiny (15 lines). Confirms ContentIndex is the only public surface.
- Severity: **LOW** (no action).

#### L2. `compact::tool_aware_chunker.rs:62` — `pub struct SemanticChunk` is returned by `chunker.rs` but only used by `summary_source.rs` and `compactor.rs`
- Path: `parse_semantic_units → ToolAwareChunker::chunk → SemanticChunk → caller`. The `Chunker` is the only producer.
- Severity: **LOW**.

#### L3. `rescue::is_silent_truncated_overflow` is private (no `pub`)
- Verified: used only inside `rescue.rs:190`. The corresponding `MAX_REACTIVE_COMPACT_ATTEMPTS` is `pub` (consumed by tests). The `fn is_silent_truncated_overflow` private status is correct — it is implementation detail of the rescue flow.
- Severity: **LOW** (no action).

#### L4. `retrieval::content_index.rs` — 1380 lines, single file
- The `ContentIndex` is the sole public item. Sub-features (BM25 search, indexed-stem prefilter, async writeback) could be split. Done **without splitting** because the public surface is a single struct + its `open` constructor + `search`/`index` methods.
- Severity: **LOW** (stylistic).

### Findings (no-severity)

#### N1. `use crate::providers::message::UnifiedMessage` in `src/context/budget/mod.rs:17`
- `UnifiedMessage` is a provider-layer type. The `context::budget` module depends on it. This is a **deliberate layering** — the budget estimates message tokens, so it must understand message shape. The dependency is narrow and one-way (`context::budget → providers::message`). No reverse coupling.
- Severity: **NONE** (defensible).

#### N2. `compact::compactor::ContextCompactor` is invoked through `compact_tool.rs` and `subagent_spawner.rs` only
- The `compact_tool.rs` is the user-facing manual `/compact` handler. The `subagent_spawner.rs` builder builds one per child session. Other paths reach `ContextCompactor` through `session_split::summarize_pretail` and `summary_source::try_reuse_session_summary`. Multi-callers, but the wiring is centralized.
- Severity: **NONE**.

## Architecture (R1-R10) check

- **R1 (Core no platform APIs)**: ✅ Clean. No `std::process::Command`, no `AppKit`, no `tokio::fs::write`-backed state.
- **R2 (Complex UI in Leptos only)**: N/A.
- **R3 (Core minimalism)**: ✅ Uses only `serde`, `tokio`, `tracing`, `anyhow`, `async-trait`, `parking_lot`-style mutex. No heavy deps.
- **R4 (Pure I/O shell)**: ✅ The context modules contain business logic (the budget algorithm) but they are the "core" of the context subsystem — not an interface shell. The harness/test consumers are the wires.
- **R5-R10**: ✅.

## Production-grade patterns observed

- `compact::compactor::ContextCompactor::with_cache_carryover` uses a 16-slot LRU keyed by session_id. Same key as the `notes` cache, so terminal expansion reuse is consistent.
- `compact::session_split::perform_session_split` has explicit `started`-time clock invariants: `started` is taken **after** the caller takes its own `Instant`, so the rescue's retries race against the same turn budget.
- `budget::ContextBudget::note_compaction_effect` smooths EWMA on the calibration factor with `CALIBRATION_ALPHA = 0.3` and clamps each observation to `[CALIBRATION_MIN, CALIBRATION_MAX] = [0.25, 4.0]`. The clamp prevents a single bad observation from whipsawing the budget.
- `compact::session_split::event_to_message` is a 1:1 mapping between event types and wire messages; the **public comment** on this function explains the marker semantics.
- `retrieval::ContentIndex::open` reads from a sqlite DB created at `index.db` — the file path is set in `tools/result_store.rs` startup. This is the only writer path.

## Conclusion

- **No HIGH-severity bugs** in `src/context/`. The wiring is clean: every public type has a consumer, and every consumer reaches a real producer.
- **M2-class MEDIUM issue**: `pressure::{detect_content_ratio, estimate_tokens_smart, chars_for_token_budget, chars_for_result_token_budget}` are `pub fn` but only used inside `pressure.rs`. Reduce visibility to `pub(crate)` to make the API contract honest.
- **M1 (LOW)**: `strip_analysis_block` and `IDENTIFIER_PRESERVATION` are re-exported from `compact::mod` *and* declared `pub` in `summary_utils.rs`. Pick one path of public exposure to prevent drift.
- No new HIGH-severity wiring gaps. The rescue system, the session-split pathway, and the budget pressure sensor each have a single canonical entry point that the harness reaches.

### Recommended fixes

1. **M2**: Tighten `pub fn` → `pub(crate) fn` (or `pub(super) fn`) for the four functions in `src/context/budget/pressure.rs` that are only used within the `pressure.rs` module.
2. **M1**: Remove the `pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION}` re-export from `src/context/compact/mod.rs` **or** remove the `pub` on `summary_utils.rs:103` (and friends). Recommend keeping the re-export and downgrading the source to `pub(crate)`.
