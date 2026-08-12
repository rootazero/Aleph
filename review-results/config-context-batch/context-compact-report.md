# Severed-Wire Audit — `src/context/compact/`

Static, read-only review of the LLM-based context-compaction subsystem: 11 files,
6,610 lines. Scope: `mod.rs`, `compactor.rs`, `manual.rs`, `session_split.rs`,
`summary_utils.rs`, `rescue.rs`, `fit.rs`, `plan_carry.rs`, `tool_aware_chunker.rs`,
`preserve.rs`, `directive.rs`.

## Summary

This module is in unusually good health. Every producer/consumer pair checked in
Phase 1–3 below is fully wired — builder methods, the rescue trait, the manual
compact wiring, session-split, and the preservation/plan-carry drain-site
invariants all resolve to live, tested call sites. The one recurring defect class
found is **dead module-level re-exports** in `mod.rs`: two `pub use` lines that
zero call sites anywhere in the tree actually use, because every real consumer
imports through the submodule path directly. No stubs (`todo!`/`unimplemented!`)
exist anywhere in the module.

---

## Phase 1 — Seam scan results

### 1. `ContextCompactor` builder parity — CLEAN

All four builder methods have live, non-test production callers:

| Builder | Producer | Consumer |
|---|---|---|
| `with_monitor_scope` | `compactor.rs:225` | `runner_impl.rs:560`, `subagent_spawner/mod.rs:1164` |
| `with_cache_carryover` | `compactor.rs:249` | `runner_impl.rs:553` |
| `with_summary_reuse` | `compactor.rs:260` | `runner_impl.rs:583`, `subagent_spawner/mod.rs:1138` (doc) |
| `with_cheap_provider` | `compactor.rs:274` | `runner_impl.rs:592` |

`summarizer_name()` and `monitor_scope()` are `#[cfg(test)]`-only accessors,
correctly gated — no production leak.

### 2. Manual compact parity — CLEAN

`manual::compact_session` is reached by exactly the two surfaces the module doc
claims and no more, both through a single shared, non-duplicated helper:

- `builtin_tools/sessions/compact_tool.rs::run_manual_compaction` (model tool call
  + Panel/channel slash command dispatch), which builds `ContextCompactor` from
  `manual::manual_summarizer()` / `CompactorConfig::default()`.
- `gateway/handlers/session/db_handlers/modify.rs::handle_compact_db` (the
  `session.compact` RPC — TUI `/compress`, CLI `aleph session compact`) calls
  `crate::builtin_tools::sessions::run_manual_compaction` directly — **the same
  function**, not a re-implementation. No drift risk here: there is structurally
  only one code path from either surface into `compact_session`.
- `install_manual_compaction` / `ManualCompactWiring` is installed once at boot in
  `bin/aleph-server/commands/start/orchestrator_init.rs:300-301`. `manual_summarizer()`
  and `manual_keep_tokens()` are both read back from the two call sites above.

### 3. Rescue parity — CLEAN

`try_reactive_compact_and_retry` / `drain_context_overflow` /
`MAX_REACTIVE_COMPACT_ATTEMPTS` are all consumed in `harness/agent/think.rs`
(lines 590, 606, 649, 722, 742), and `AgentHarness` implements `RescueHost` at
`think.rs:1410`. Verified specifically:

- `MAX_REACTIVE_COMPACT_ATTEMPTS` is **not** decorative — `harness/agent.rs:197`
  reads the constant from `context::compact::rescue` rather than hard-coding `1`
  (this was previously a known failure mode per `src/harness/CLAUDE.md`'s history;
  it is fixed and stayed fixed).
- The `RescueCx.started` clock invariant (see Rules section below — the audit
  brief names this on `perform_session_split`, which is a naming slip; the
  invariant actually lives on `RescueCx` in `rescue.rs`) holds: `think.rs:531`
  takes `Instant::now()` and `think.rs:535` constructs `RescueCx` immediately
  after with that same instant, and `cx.started` is consumed by
  `race_llm_call` at `think.rs:1429` — so rescue retries race the *same* turn
  budget as the primary call, exactly as documented.

### 4. Session-split parity — CLEAN

`perform_session_split` has exactly one production call site,
`directive.rs:149` inside the `LoopDirective::SplitSession` arm of
`apply_budget_directive`, which is itself called from `harness/agent/think.rs:460`
and its `DirectiveOutcome::SplitTo` / `FellThrough` arms are both handled
(`think.rs:475`, `483`). Test-only extra call sites in
`session_split.rs`'s own `#[cfg(test)]` module are appropriately scoped.

Note: the audit brief also asks to check a call site in
`tests/.../extras.rs` — no such file exists under that name in this tree; the
only test consumer is `session_split.rs`'s own inline test module. Not a defect,
just a stale instruction in the audit brief.

### 5. Summary-utils parity — MIXED (see Finding F1 below)

`strip_analysis_block` / `IDENTIFIER_PRESERVATION` are consumed correctly by
`compactor.rs`, `memory/session_compactor/summary_engine.rs`,
`memory/session_reflection/mod.rs`, `memory/session_search_summary/synthesizer.rs`
— but **every one of them imports the submodule path
`context::compact::summary_utils::{...}` directly**, never the module-level
re-export `context::compact::{strip_analysis_block, IDENTIFIER_PRESERVATION}`
declared at `mod.rs:27`. See Finding F1.

The same double-exposure pattern recurs for the `tool_aware_chunker` re-export at
`mod.rs:28` — see Finding F2.

`cap_summary_lines` / `clamp_start_to_budget` are `pub(crate)` (not exported from
`mod.rs` at all) and are correctly consumed by `manual.rs` and `session_split.rs`
directly via `summary_utils::{...}` — this part of the design is fine and matches
the doc comment "single-sourced here because all three drain sites need the same
number."

### 6. Stub sweep — CLEAN

No `TODO`, `unimplemented!`, `todo!`, `FIXME`, or `XXX` anywhere in
`src/context/compact/`. `tool_aware_chunker.rs`'s own module doc records that a
previous round of *this same discipline* already removed unreachable
tool-pairing machinery from that file, which is a healthy sign, not a red flag.

---

## Phase 2 — Enumerated candidates

| # | Producer | Consumer | Status |
|---|---|---|---|
| C1 | `compactor.rs:225,249,260,274` (builders) | `runner_impl.rs:553-592`, `subagent_spawner/mod.rs:1164,1172` | CONNECTED |
| C2 | `rescue.rs:55` `MAX_REACTIVE_COMPACT_ATTEMPTS` | `harness/agent.rs:197` | CONNECTED |
| C3 | `rescue.rs:180,248` `drain_context_overflow`/`try_reactive_compact_and_retry` | `harness/agent/think.rs:590-742` | CONNECTED |
| C4 | `directive.rs:74` `apply_budget_directive` | `harness/agent/think.rs:460-483` | CONNECTED |
| C5 | `session_split.rs:50` `perform_session_split` | `directive.rs:149` | CONNECTED |
| C6 | `manual.rs:208` `compact_session` | `compact_tool.rs:128`, `modify.rs:607` (via shared `run_manual_compaction`) | CONNECTED |
| C7 | `manual.rs:175` `install_manual_compaction` | `orchestrator_init.rs:300` | CONNECTED |
| C8 | `preserve.rs:78` `preserved_user_messages` | `compactor.rs:469,549,678,769` (all 4 documented drain sites) | CONNECTED |
| C9 | `plan_carry.rs:69` `plan_carry_message` | `compactor.rs:860` (`splice_preserved`, itself 3 call sites) | CONNECTED |
| C10 | `fit.rs:112` `compact_to_fit` | `directive.rs:50` (`compact_to_fit_and_note`) | CONNECTED |
| **F1** | `mod.rs:27` `pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION}` | **none** — all 4 external consumers use `summary_utils::` path directly | **CUT candidate** |
| **F2** | `mod.rs:28` `pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker}` | **none** — sole consumer (`session_compactor/post_turn_compress.rs`, `chunker_tests.rs`) uses `tool_aware_chunker::` path directly | **CUT candidate** |

---

## Phase 3 — Triage detail

### F1 — Dead re-export: `compact::strip_analysis_block` / `compact::IDENTIFIER_PRESERVATION`

- **Producer**: `src/context/compact/mod.rs:27`
  ```rust
  pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
  ```
- **Consumer**: none via this path. Grepped every call site of both symbols across
  the tree:
  - `context/compact/compactor.rs:13` — `use super::summary_utils::{..., strip_analysis_block, ...}`
  - `memory/session_compactor/summary_engine.rs:8,12` — `use crate::context::compact::summary_utils::IDENTIFIER_PRESERVATION;` and `pub use crate::context::compact::summary_utils::strip_analysis_block;`
  - `memory/session_reflection/mod.rs:40` — `use crate::memory::session_compactor::summary_engine::strip_analysis_block;` (via the `summary_engine` re-export above, itself sourced from `summary_utils`, not `compact::`)
  - `memory/session_search_summary/synthesizer.rs:24` — imports from a local `build_summary_prompt, strip_analysis_block` module (not `context::compact` directly — different function of the same name family; not this module's export at all)
  - `memory/session_compactor/helpers.rs:38,68` — `super::summary_engine::strip_analysis_block`

  A repo-wide grep for `compact::strip_analysis_block` and `compact::IDENTIFIER_PRESERVATION`
  (i.e. anything actually resolving through the `mod.rs:27` re-export) returns **zero
  matches**.
- **Severity**: LOW
- **Triage**: **CUT**
- **Reason**: This is the same defect the previous audit flagged as M1, but the
  previous audit's proposed remedy ("keep the re-export, downgrade the source to
  `pub(crate)`") was not applied — `summary_utils::strip_analysis_block` and
  `summary_utils::IDENTIFIER_PRESERVATION` are still fully `pub`, and the `mod.rs`
  re-export sits alongside them with provably zero traffic. Since `summary_utils`
  is itself `pub mod summary_utils;` (mod.rs:24), every consumer already has direct
  access via the submodule path — the re-export adds no reachability, only a second
  name for the same two items that nothing resolves through.
- **Proposed fix**: Delete `mod.rs:27` entirely. This is a smaller, more final
  version of the previous audit's recommendation: since nothing uses the
  re-export today, there's no reachability to preserve by keeping it — CUT is
  strictly safer than the "keep + narrow visibility" compromise, and matches the
  session's `CLAUDE.md` guidance ("零消费者的通道优先 CUT，不 CONNECT").

### F2 — Dead re-export: `compact::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker}`

- **Producer**: `src/context/compact/mod.rs:28`
  ```rust
  pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
  ```
- **Consumer**: none via this path.
  - `memory/session_compactor/post_turn_compress.rs:84,87` — fully-qualified
    `crate::context::compact::tool_aware_chunker::parse_semantic_units(...)` and
    `crate::context::compact::tool_aware_chunker::ToolAwareChunker::new(...)`.
  - `memory/session_compactor/chunker_tests.rs:1-3` — `use crate::context::compact::tool_aware_chunker::{parse_semantic_units, SemanticUnit, ToolAwareChunker};`

  Same as F1: a repo-wide grep for the module-level path (`compact::parse_semantic_units`,
  `compact::ToolAwareChunker`, etc.) returns zero matches; every consumer reaches
  through `tool_aware_chunker::` explicitly.
- **Severity**: LOW
- **Triage**: **CUT**
- **Reason**: Identical shape to F1 — `tool_aware_chunker` is `pub mod` (mod.rs:25),
  so the re-export is pure duplication with zero traffic.
- **Proposed fix**: Delete `mod.rs:28`.

**Combined note on F1/F2**: both re-exports are LOW severity — they cost nothing
at runtime and are not actively misleading anyone today — but they are exactly
the "double-exposure" shape the audit brief asked to watch for, and the drift risk
is real: if a future edit changes the *re-exported* signature without touching
the submodule original (or vice versa), the two names could silently diverge with
no compiler signal, since nothing currently type-checks against the re-export to
catch it. Since the previous audit's narrower fix (downgrade source visibility)
was never applied and traffic is provably zero on both, straight deletion is the
lower-maintenance option and removes 2 lines with no reachability cost.

---

## Phase 4 — Fix recommendations (consolidated)

| Finding | Producer file:line | Consumer file:line | Severity | Triage | Reason | Proposed fix |
|---|---|---|---|---|---|---|
| F1 | `context/compact/mod.rs:27` | *(no consumer resolves through this path)* | LOW | CUT | Re-export has zero traffic; all 4 real consumers import `summary_utils::` directly | Delete line 27 |
| F2 | `context/compact/mod.rs:28` | *(no consumer resolves through this path)* | LOW | CUT | Re-export has zero traffic; the sole consumer imports `tool_aware_chunker::` directly | Delete line 28 |

No CRITICAL, HIGH, or MEDIUM findings. No CONNECT candidates — every producer in
this module has a live, non-stub consumer.

---

## Phase 5 — Guard recommendation

- **Builder methods** (`with_cache_carryover`, `with_monitor_scope`,
  `with_summary_reuse`, `with_cheap_provider`): each has 1–2 non-test production
  callers, all inside `orchestrator/harness_bridge/runner_impl.rs` and
  `agents/subagent_spawner/mod.rs`. No guard needed today — these are the two
  legitimate `ContextCompactor` construction sites (main-run bridge and subagent
  spawner) and the fan-out is small and stable. If a third construction site
  appears, it's worth re-checking that it doesn't silently skip
  `with_cache_carryover` (the carry-over slot is process-wide and keyed by session,
  so a compactor built without it just never benefits from cross-run cache reuse —
  a silent perf regression, not a correctness bug).
- **`MAX_REACTIVE_COMPACT_ATTEMPTS`**: already correctly wired (`harness/agent.rs:197`
  reads the constant, does not hard-code). No action needed; flagging here only to
  confirm the audit brief's specific "if it's pub but production reads a hardcoded
  2" scenario does **not** apply to the current code.
- **F1/F2 re-exports**: recommend adding a source-level guard analogous to the
  ones already documented in this repo's `CLAUDE.md` judgment-criteria list (e.g.
  `no_catalog_entry_inlines_its_description`) is **not** warranted here — this is
  two dead `pub use` lines, not a recurring architectural hazard, and the fix is
  a one-line deletion each. A guard would be overkill; just cut them.

---

## Notes outside the assigned scope (FYI, not part of this module's findings)

While tracing `manual::compact_session`'s doc comment (which explicitly contrasts
itself against the legacy `SessionManager::compact_session(KeepLastN{50})` path
that deletes rows from the `messages` read-projection table), I found that the
legacy path it describes as superseded — `gateway/session_manager/ops/modify.rs:37`
`SessionManager::compact_session` — **still exists and is still called
automatically** from `gateway/session_manager/ops/crud.rs:264` whenever a
session's `messages` row count exceeds `config.max_messages`. This is a different
table (`messages`, the Panel's read-projection) than the one this module's
`manual::compact_session` operates on (`session_events`, the prompt's source of
truth), so it is **not a severed wire in `src/context/compact/`** and is outside
this audit's file scope — but it does mean the Panel's visible scrollback is
still subject to silent auto-trimming on a schedule independent of, and
undocumented relative to, the `/compact` feature this module implements. Worth a
follow-up audit of `src/gateway/session_manager/` if that's in scope elsewhere.
