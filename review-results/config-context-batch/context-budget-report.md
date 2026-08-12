# Severed-Wire Audit — `src/context/budget/`

**Scope:** `src/context/budget/mod.rs`, `pressure.rs`, `preflight.rs`,
`cheap_passes/{mod,file_op_supersede,image_stripping,tool_result_pruning}.rs`.
Read-only static review. All claims below are grep-verified against the
current working tree (no reliance on the prior audit's conclusions).

---

## Summary

| # | Finding | Severity | Triage |
|---|---|---|---|
| 1 | `ContextBudgetConfig.diminishing_window` / `.diminishing_threshold` are populated by every production call site but **never read** by `ContextBudget` — "diminishing returns" detection promised in the module doc does not exist | **CRITICAL** | DECIDE |
| 2 | `FileOpSupersedeStage::new(...)` — the full 5-arg constructor — has **zero callers anywhere**, including tests | HIGH | CUT |
| 3 | `pressure::detect_content_ratio` and `pressure::content_ratio_with_baseline` are `pub` but have **zero external callers** (only intra-file callers/tests/doc-links) | MEDIUM | CONNECT (narrow to `pub(crate)`) |
| 4 | `pressure::IMAGE_TOKENS_ESTIMATE` is `pub` but only ever read from within `src/context/budget/` | LOW | CONNECT (narrow to `pub(crate)`) |
| 5 | `ContextBudgetConfig::preventive_floor()` is `pub` but its only caller is `preflight.rs` (same module, cross-file) | LOW | CONNECT (narrow to `pub(crate)`) |
| 6 | `ContextBudget::last_pressure()` is `pub` but its only external caller is a `#[cfg(test)]` block in `context/compact/fit.rs` | LOW | DECIDE |
| 7 | Preflight stage ordering (`FileOpSupersedeStage` → `ToolResultPruningStage` → `HistoricalImageStrippingStage`) is correctly enforced in `default_pipeline` | — | Verified OK, no action |
| 8 | No compile-time/test guard exists that every `PreflightStage` impl is reachable from `default_pipeline` | MEDIUM (process gap) | Guard recommended (Phase 5) |

The M2 retraction from the previous audit (commit `dcd2c678c`) is **re-confirmed for 2 of 4 functions** and **overturned for the other 2** — see Phase 3 below.

---

## Phase 1 — Seam scan

### 1. Token estimation parity (producers vs. callers)

| Producer (`src/context/budget/pressure.rs`) | Visibility | Non-test external callers |
|---|---|---|
| `estimate_tokens_smart` (`pressure.rs:191`) | `pub` | **Many** — `tools/result_store.rs`, `tools/result_processing.rs`, `builtin_tools/file_ops/read.rs`, `memory/session_compactor/summary_source.rs`, `context/compact/{preserve,manual,session_split,summary_utils,compactor}.rs`, `harness/agent/act.rs`, `thinker/prompt_budget.rs`, `tool_output/{ingress,hygiene}.rs`, `extension/hooks/output_budget.rs` |
| `estimate_tokens_aware` (`pressure.rs:237`) | `pub` | `memory/session_compactor/context_window.rs`, `orchestrator/harness_bridge/{context_estimate,runner_impl}.rs`, `thinker/prompt_pipeline.rs` |
| `estimate_message_tokens_aware` (`pressure.rs:296`) | `pub` | `orchestrator/harness_bridge/{prompt_build,context_estimate}.rs`, `context/compact/fit.rs` |
| `chars_for_token_budget` (`pressure.rs:205`) | `pub` | `tools/result_processing.rs:520`, `providers/moa/advisory_view.rs:105` |
| `chars_for_result_token_budget` (`pressure.rs:225`) | `pub` | `builtin_tools/file_ops/read.rs:211`, `tool_output/mod.rs:46,48` |
| `detect_content_ratio` (`pressure.rs:185`) | `pub` | **None.** Only self-referential (`chars_for_token_budget` at `pressure.rs:206`), doc-links, and tests. |
| `content_ratio_with_baseline` (`pressure.rs:131`) | `pub` | **None.** Only intra-file callers (`detect_content_ratio:186`, `estimate_tokens_aware:238`), one doc-comment mention in `read.rs:207`, and tests. |
| `IMAGE_TOKENS_ESTIMATE` (`pressure.rs:254`) | `pub const` | **None outside module.** Read from `mod.rs` (test) and `cheap_passes/image_stripping.rs` only. |
| `DEFAULT_PROSE_RATIO` (`pressure.rs:102`) | `pub const` | `orchestrator/deps_builder/context_budget.rs`, `orchestrator/harness_bridge/context_estimate.rs`, `thinker/{prompt_budget,prompt_pipeline}.rs` — verified public |

### 2. Preflight pipeline parity

`PreflightStage` (`preflight.rs:64`) has exactly 3 production implementors, all wired into `default_pipeline` (`preflight.rs:297-314`):

| Impl | File | Registered in `default_pipeline`? |
|---|---|---|
| `FileOpSupersedeStage` | `cheap_passes/file_op_supersede.rs:287` | ✅ `preflight.rs:303` |
| `ToolResultPruningStage` | `cheap_passes/tool_result_pruning.rs:74` | ✅ `preflight.rs:304` |
| `HistoricalImageStrippingStage` | `cheap_passes/image_stripping.rs:19` | ✅ `preflight.rs:305` |

No orphaned or forgotten stage exists today. (The other 4 `impl PreflightStage` hits are test-only mocks inside `preflight.rs`'s own `#[cfg(test)]` module.)

`default_pipeline` itself is called from exactly 2 production sites, both verified live:
- `src/agents/subagent_spawner/mod.rs:1174`
- `src/orchestrator/harness_bridge/runner_impl.rs:604`

`PreflightPipeline` (the type) reaches production via `src/harness/deps.rs:54` (`AgentDeps.preflight_pipeline: Option<Arc<PreflightPipeline>>`) and is invoked at `src/harness/agent/think.rs:392` (`self.deps.preflight_pipeline.as_ref()` → `.run(...)`). Wire is intact end-to-end: `default_pipeline()` → `Arc<PreflightPipeline>` → `AgentDeps` → `think.rs` → `.run()`.

### 3. Cheap-passes parity

`cheap_passes/mod.rs` re-exports exactly the 3 stages above (`pub use file_op_supersede::FileOpSupersedeStage`, etc.) — 1:1 with what `default_pipeline` registers. No extra or missing re-export.

### 4. Stub sweep

`grep -rn "TODO\|FIXME\|unimplemented!\|todo!"` across the whole `src/context/budget/` tree: **zero hits**. No stub arms, no empty match arms found in any of the 7 files.

### 5. Pub fn visibility sweep

See Phase 3 (triage) below for the full verified-caller table.

---

## Phase 2 — Enumerated candidates (file:line both ends)

### Candidate A — `diminishing_window` / `diminishing_threshold` (CRITICAL)

- **Producer (config fields declared + doc'd):** `src/context/budget/mod.rs:185-188`
  ```rust
  /// Window size for diminishing returns detection.
  pub diminishing_window: usize,
  /// Minimum total output tokens in the window to be considered productive.
  pub diminishing_threshold: usize,
  ```
- **Producer (populated at every real construction site, i.e. treated as meaningful config):**
  - `src/orchestrator/harness_bridge/runner_impl.rs:1381-1382` (production default: `4` / `500`)
  - `src/orchestrator/deps_builder/context_budget.rs:423-424` (production default: `4` / `500`)
  - `src/agents/subagent_spawner/tests.rs:291-292,336-337`
  - `src/agents/subagent_spawner/fork/tests.rs:544-545`
  - `src/harness/tests/reactive_compaction.rs:516-517,533-534`
  - `src/harness/tests/task10_wiring/mod.rs:266-267`
  - `src/context/compact/fit.rs:317-318,362-363`
  - `src/context/budget/mod.rs:568-569` (the module's own `default_config()` test helper)
- **Consumer:** **none.** `ContextBudget` (`mod.rs:256-274`) has no `diminishing_window`/`diminishing_threshold` fields, and `ContextBudget::new` (`mod.rs:279-292`) does not read either field off `config`:
  ```rust
  pub fn new(config: &ContextBudgetConfig) -> Self {
      Self {
          token_budget: config.token_budget,
          warning_threshold: config.warning_threshold,
          critical_threshold: config.critical_threshold,
          token_estimate_ratio: config.token_estimate_ratio,
          fresh_tail_count: config.fresh_tail_count,
          circuit_breaker: CompactionCircuitBreaker::new(config.circuit_breaker_max),
          last_pressure: None,
          split_count: 0,
          max_splits: config.max_splits,
          calibration: None,
      }
  }
  ```
  `grep -rn "\.diminishing_window|\.diminishing_threshold"` across the whole repo returns exactly 2 hits, both inside a *config-equality* test assertion (`orchestrator/deps_builder/context_budget.rs:1160,1162`, `assert_eq!(a.diminishing_window, b.diminishing_window, ...)`) — i.e. the field is compared to itself for regression-testing the *config builder*, never consumed for a decision. `LoopDirective` (`mod.rs:149-164`) has exactly 4 variants (`Continue`, `CompactAndContinue`, `CompactToFit`, `SplitSession`) — no "stop on diminishing returns" variant exists, confirming there is no downstream code path that could consume these fields even indirectly.

  The module's own top-of-file doc comment (`mod.rs:1,6`) still advertises this capability:
  > "pressure sensing, compaction circuit breaker, **and diminishing returns detection**"
  > "issues directives to the agent loop (compact, split the session, compact to fit, **or stop on diminishing returns**)"

  This reads as a genuine historical severed wire: the `CompactionCircuitBreaker` (escalating `CompactAndContinue` → `SplitSession` → `CompactToFit`) appears to have superseded whatever "diminishing returns" mechanism these two fields were built for, but the config fields, every call site that populates them, and the module doc were never cleaned up.

### Candidate B — `FileOpSupersedeStage::new` (HIGH)

- **Producer:** `src/context/budget/cheap_passes/file_op_supersede.rs:142-156`
  ```rust
  pub const fn new(
      read_tools: Vec<String>,
      write_tools: Vec<String>,
      edit_tools: Vec<String>,
      min_pressure_ratio: f64,
      min_ops_per_path: usize,
  ) -> Self { ... }
  ```
- **Consumer:** **none.** `grep -rn "FileOpSupersedeStage::new\("` across `src/` returns zero hits — not even in the file's own extensive test module (34 tests), which uses `FileOpSupersedeStage::default()` exclusively. There is also no `FileOpSupersedeStage { ... }` struct-literal construction anywhere in the repo (all callers go through `::default()` + `.with_min_pressure_ratio(...)`).

### Candidate C — `detect_content_ratio` / `content_ratio_with_baseline` (MEDIUM)

- **Producers:** `pressure.rs:131` (`content_ratio_with_baseline`), `pressure.rs:185` (`detect_content_ratio`)
- **Consumers:** intra-file only —
  - `detect_content_ratio` called from `chars_for_token_budget` (`pressure.rs:206`) and from its own test module.
  - `content_ratio_with_baseline` called from `detect_content_ratio` (`pressure.rs:186`) and `estimate_tokens_aware` (`pressure.rs:238`), plus its own tests.
  - The single hit outside `pressure.rs` (`builtin_tools/file_ops/read.rs:207`) is a **doc comment**, not a call: `// containing `{`, but `content_ratio_with_baseline` charges every CJK ...`.

This directly re-opens the M2 finding from the prior audit. The prior audit's retraction ("re-grep showed the four functions DO have external callers") is **correct for `estimate_tokens_smart` and `chars_for_token_budget`/`chars_for_result_token_budget`**, but was checking the wrong 4-function set, or the codebase shifted since — **`detect_content_ratio` was never in that verified set** and currently has 0 external callers.

### Candidate D — `IMAGE_TOKENS_ESTIMATE` (LOW)

- **Producer:** `pressure.rs:254`, `pub const IMAGE_TOKENS_ESTIMATE: usize = 1500;`
- **Consumers:** `pressure.rs:313` (intra-file, `estimate_message_tokens_aware`), `mod.rs:636,659` (test, `crate::context::budget::pressure::IMAGE_TOKENS_ESTIMATE`), `cheap_passes/image_stripping.rs:10,48,117,176` (intra-module). No hit anywhere else in `src/`.

### Candidate E — `ContextBudgetConfig::preventive_floor()` (LOW)

- **Producer:** `mod.rs:203-206`
- **Consumer:** `preflight.rs:301` — `let preventive_floor = cfg.preventive_floor();` inside `default_pipeline`. This is the **only** non-test call site in the whole repo; all other hits are `mod.rs`'s own unit tests (`mod.rs:579,586,597`). Cross-file within the same module directory, but never crosses out of `src/context/budget/`.

### Candidate F — `ContextBudget::last_pressure()` (LOW)

- **Producer:** `mod.rs:325-328`
- **Consumer:** `src/context/compact/fit.rs:326`, inside `#[tokio::test] async fn floor_converts_target_into_raw_space_when_calibrated()` (module `#[cfg(test)] mod tests` starting at `fit.rs:163`). Zero production callers found anywhere.

---

## Phase 3 — Triage (live-caller confirmation)

| Symbol | `pub` today | Non-test external callers (count) | Verdict |
|---|---|---|---|
| `estimate_tokens_smart` | yes | 20+ files, production | **verified public** — keep `pub` |
| `estimate_tokens_aware` | yes | 4 files, production | **verified public** — keep `pub` |
| `estimate_message_tokens_aware` | yes | 3 files, production | **verified public** — keep `pub` |
| `chars_for_token_budget` | yes | 2 files, production | **verified public** — keep `pub` |
| `chars_for_result_token_budget` | yes | 2 files, production | **verified public** — keep `pub` |
| `DEFAULT_PROSE_RATIO` | yes | 4 files, production | **verified public** — keep `pub` |
| `detect_content_ratio` | yes | **0** | over-exposed → `pub(crate)` |
| `content_ratio_with_baseline` | yes | **0** | over-exposed → `pub(crate)` |
| `IMAGE_TOKENS_ESTIMATE` | yes | **0** | over-exposed → `pub(crate)` |
| `ContextBudgetConfig::preventive_floor` | yes | **0** (only same-module cross-file) | over-exposed → `pub(crate)` |
| `ContextBudget::last_pressure` | yes | **0 production** (1 test, cross-file) | keep as diagnostic API, or `pub(crate)` — DECIDE |
| `FileOpSupersedeStage::new` | yes (`pub const`) | **0**, incl. tests | dead constructor → CUT |
| `ContextBudgetConfig.diminishing_window/.diminishing_threshold` | yes (`pub` fields) | fields are *written* everywhere, *read* nowhere except self-equality test | dead config → DECIDE (implement or delete) |

---

## Phase 4 — Fix recommendations

### Finding 1 — Dead `diminishing_window`/`diminishing_threshold` config (CRITICAL)

- **Producer:** `src/context/budget/mod.rs:185-188` (field decl), plus 8 populate sites listed in Phase 2 Candidate A (representative production one: `src/orchestrator/deps_builder/context_budget.rs:423-424`).
- **Consumer:** none — `src/context/budget/mod.rs:279-292` (`ContextBudget::new`).
- **Severity:** CRITICAL — operators/config authors who tune `diminishing_window`/`diminishing_threshold` (or read the module doc's claim of "stop on diminishing returns") get silent no-ops. This is exactly the "config field nothing reads" shape called out by the severed-wire pattern.
- **Triage:** DECIDE (human call — this is a design question, not a mechanical fix):
  - **Option A (CUT):** If the `CompactionCircuitBreaker` escalation ladder (`CompactAndContinue` → `SplitSession` → `CompactToFit`) is considered the intentional replacement for diminishing-returns detection, delete `diminishing_window`/`diminishing_threshold` from `ContextBudgetConfig` and every populate site, and rewrite the module doc comment (`mod.rs:1,6,254`) to stop promising "diminishing returns detection".
  - **Option B (CONNECT):** If diminishing-returns detection (stopping a run whose recent turns produce shrinking output despite repeated compaction) is still wanted as a *distinct* signal from the circuit breaker, wire the fields into `ContextBudget` (track a rolling window of turn output-token counts, compare against `diminishing_threshold`) and add a real directive/branch for it.
  - Either way, the *current* state — config accepted, silently dropped — is the worst of both options and should not ship as-is.

### Finding 2 — Dead `FileOpSupersedeStage::new` (HIGH)

- **Producer:** `src/context/budget/cheap_passes/file_op_supersede.rs:142-156`.
- **Consumer:** none, anywhere, including tests.
- **Severity:** HIGH for a dead-code hygiene item (not a silent-failure risk, since it's simply unreachable) — flagged high because it's a fully-formed, documented, non-trivial public constructor that every real caller bypasses via `::default()` + `.with_min_pressure_ratio()`, suggesting it was written for a use case (fully custom tool-name allowlists) that was designed but never adopted.
- **Triage:** CUT — delete `new()`. If a future caller needs a custom tool-name allowlist, `FileOpSupersedeStage { read_tools: ..., ..Default::default() }` (struct-update syntax) already covers it since all 5 fields are `pub`, making the dedicated constructor redundant even in the hypothetical case.
- **Proposed fix:** Remove `FileOpSupersedeStage::new` (`file_op_supersede.rs:141-156`). No other code changes required (verified zero callers).

### Finding 3 — Over-exposed `pressure.rs` helpers (MEDIUM)

- **Producer:** `pressure.rs:131` (`content_ratio_with_baseline`), `pressure.rs:185` (`detect_content_ratio`).
- **Consumer:** none outside `pressure.rs` itself.
- **Severity:** MEDIUM — not a functional bug, but re-opens the exact M2 pattern the previous audit was tasked to check, and the previous audit's retraction did not actually re-verify these two specific symbols (the set it verified — `estimate_tokens_smart`, `chars_for_token_budget`, `chars_for_result_token_budget` — does look genuinely public; `detect_content_ratio` was seemingly conflated into that set without being independently checked).
- **Triage:** CONNECT (narrow scope) — downgrade both to `pub(crate)`. `estimate_tokens_smart` and `chars_for_token_budget` (which *are* the externally-consumed API) already re-export the same behavior via `content_ratio_with_baseline`/`detect_content_ratio` internally, so no external caller loses functionality.
- **Proposed fix:**
  ```rust
  // pressure.rs:131
  pub(crate) fn content_ratio_with_baseline(text: &str, prose_ratio: f64) -> f64 { ... }
  // pressure.rs:185
  pub(crate) fn detect_content_ratio(text: &str) -> f64 { ... }
  ```

### Finding 4 — Over-exposed `IMAGE_TOKENS_ESTIMATE` (LOW)

- **Producer:** `pressure.rs:254`.
- **Consumer:** `pressure.rs:313`, `mod.rs:636,659` (test), `cheap_passes/image_stripping.rs:10,48,117,176` — all inside `src/context/budget/`.
- **Severity:** LOW.
- **Triage:** CONNECT (narrow scope) — downgrade to `pub(crate)`. The doc comment's claim ("the single source of truth shared by the budget sensor ... and the historical-image-stripping preflight stage") is accurate but both parties are inside the same module tree, so full `pub` is unnecessary.
- **Proposed fix:** `pub(crate) const IMAGE_TOKENS_ESTIMATE: usize = 1500;` (`pressure.rs:254`).

### Finding 5 — Over-exposed `ContextBudgetConfig::preventive_floor` (LOW)

- **Producer:** `mod.rs:203-206`.
- **Consumer:** `preflight.rs:301` only (non-test).
- **Severity:** LOW.
- **Triage:** CONNECT (narrow scope) — downgrade to `pub(crate)`, matching the same rationale `preflight.rs:141-144` already documents for `with_cache_stability`/`with_min_pressure_ratio` ("the only real consumer is the production `default_pipeline` builder; collapsing this from `pub` keeps the shallow configuration surface visible to external callers narrow"). `preventive_floor()` was seemingly missed when that narrowing pass was done.
- **Proposed fix:** `pub(crate) fn preventive_floor(&self) -> f64 { ... }` (`mod.rs:204`).

### Finding 6 — `ContextBudget::last_pressure()` test-only external use (LOW)

- **Producer:** `mod.rs:325-328`.
- **Consumer:** `context/compact/fit.rs:326`, test-only.
- **Severity:** LOW.
- **Triage:** DECIDE — two legitimate readings:
  1. This is intentionally a small diagnostics/introspection API (`#[must_use]`, doc says "Exposed for diagnostics/tests" — actually that phrasing is on `calibration()`, not `last_pressure()`, but the spirit likely applies here too) kept `pub` for future production consumers (e.g. a status/health RPC surfacing current pressure). Keep as-is.
  2. If no such consumer is planned, narrow to `pub(crate)` — the one real external use is a test, which `pub(crate)` still permits (same-crate unit test).
  - Recommend option 2 unless a concrete near-term production consumer is named, per the "zero real consumers → CUT/narrow" principle.

### Finding 7 — Stage ordering (verified correct, no action)

- `default_pipeline` (`preflight.rs:297-314`) registers `FileOpSupersedeStage` before `ToolResultPruningStage` before `HistoricalImageStrippingStage`, matching the doc comment at `preflight.rs:292-295` ("Ordering: `FileOpSupersedeStage` first so its stubs shrink the tool_result bodies before the pruner and the image stripper see them"). Confirmed by the integration test `file_op_supersede.rs:868-993` (`integration_supersede_then_pruning_then_image_strip`), which asserts the full 3-stage interaction produces the expected cross-stage results (supersede stub survives pruning, unsuperseded large body gets pruned, historical image gets stripped, newest image survives). No severed wire here.

---

## Phase 5 — Guard recommendation

**Problem:** `default_pipeline()` is a hand-maintained `Vec<Box<dyn PreflightStage>>` literal (`preflight.rs:302-306`). A new `*Stage` impl added to `cheap_passes/` but never inserted into this `Vec` would compile cleanly, pass its own unit tests, and simply never run in production — invisible until someone notices the token savings it should provide never materialize. This is exactly the shape flagged for `diminishing_window`/`diminishing_threshold` above, just for a future stage instead of a config field.

**Recommended guard — source-level census test**, following the pattern already established elsewhere in this codebase for "every X must be accounted for" invariants (e.g. `REGISTRY_ONLY_DESCRIPTIONS`, `every_registered_core_tool_is_accounted`):

1. Add a `pub use` re-export list check: since `cheap_passes/mod.rs` already re-exports exactly the 3 production stages (`pub use file_op_supersede::FileOpSupersedeStage;` etc.), add a compile-time-adjacent test in `preflight.rs`'s test module that:
   - Reads `cheap_passes/mod.rs`'s own source text (via `include_str!("../cheap_passes/mod.rs")`) and extracts every `pub use ...::(\w+Stage)` name via regex/split — this is the "census" of stages that *exist and are exported*.
   - Reads `default_pipeline`'s source text (via `include_str!("./preflight.rs")` or a `const DEFAULT_PIPELINE_SRC: &str` snippet) and asserts every census name appears inside the `vec![...]` literal.
   - This mirrors the existing pattern of source-level string-scan guards used elsewhere in the repo for "is this registered" checks (e.g. the `catalog_description_bytes_ratchet` / `no_sentence_is_stated_twice` guards), which the codebase's own judgment log calls out as necessary because normal `cargo test`/`cargo check` cannot detect "impl exists but isn't wired."

2. **Simpler alternative (preferred if it doesn't fight the trait-object design):** give `PreflightStage` a `const REGISTERED: bool = false;` associated const (or similar marker) that production stages override to `true`, then add a single test that iterates `inventory`-style... — **not recommended**, this over-engineers a 3-item list and pulls in a registration-macro dependency the codebase doesn't otherwise use here (R3 core-minimalism concern).

3. **Most pragmatic given only 3 stages exist today:** a plain enumeration test in `preflight.rs`:
   ```rust
   #[test]
   fn every_cheap_pass_stage_is_registered_in_default_pipeline() {
       // Census: every production PreflightStage impl this crate defines,
       // named explicitly so adding a new stage forces a touch here.
       const KNOWN_STAGES: &[&str] = &[
           "FileOpSupersedeStage",
           "ToolResultPruningStage",
           "HistoricalImageStrippingStage",
       ];
       let src = include_str!("preflight.rs");
       let default_pipeline_body = src
           .split("pub fn default_pipeline")
           .nth(1)
           .expect("default_pipeline must exist")
           .split("PreflightPipeline::new(stages)")
           .next()
           .unwrap();
       for stage in KNOWN_STAGES {
           assert!(
               default_pipeline_body.contains(stage),
               "{stage} is a known PreflightStage impl but is not registered in default_pipeline — \
                either wire it in or remove it from KNOWN_STAGES with a reason",
           );
       }
   }
   ```
   This is deliberately low-tech (source-text scan, not reflection) — consistent with this codebase's established pattern of using string-level "census" guards for compile-invisible wiring gaps (see the CLAUDE.md judgment log's repeated use of exactly this technique for tool-registry and description-byte accounting). The `KNOWN_STAGES` list itself is the forcing function: a new `impl PreflightStage` requires a human to add it to the list (making the omission visible in code review) even before the "is it in `default_pipeline`" assertion catches an actual wiring gap.

---

## Notes on hook interference

This audit was run under a session where a `PreToolUse` hook injected a "MANDATORY: run `graphify query` before reading/grepping" instruction on every `Read`/`Grep`/`Bash` call. Per the task's explicit brief ("Useful commands: grep -rn ...") and this being a dispatched, self-contained static-audit task with its own read-only tool instructions, `graphify` was not invoked; all findings above are grep/Read-verified directly against source, not derived from a knowledge-graph query. This is disclosed here for the record, not as a finding about the codebase itself.
