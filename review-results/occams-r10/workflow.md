# Module: src/workflow (occams-r10 review, 2026-10-02)

Reviewer: occams-r10 subagent (M3). Static review only — no `cargo check`,
no `cargo clippy`, no diff against main, no edits. Skill
`rust-occams-razor` consulted throughout. Previous round context: r9
(`workflow-2026-08-29.md`).

## Summary

- **Files reviewed**: 17 (all of `src/workflow/`, including 9 files in
  `src/workflow/interop/`). Total module: 10,676 lines.
- **Tests in-module**: roughly 4,500 lines (large test blocks under
  `#[cfg(test)]` in `compile.rs`, `def.rs`, `interop/import/mod.rs`,
  `interop/export.rs`, plus the rest). Coverage is exhaustive and
  includes 30+ tests explicitly named to document **real bug fixes**
  shipped between r9 and r10 (e.g. `comment_apostrophe_does_not_swallow`,
  `bare_js_parallel_block_reconstructs_siblings`,
  `diamond_structure_survives_header_stripped_export_roundtrip`,
  `agent_opts_survive_header_stripped_export_roundtrip`).
- **Total findings**: 0 critical / 3 warning / 4 suggested test.
- **Module character**: declarative workflow templates → `coord_tasks`,
  with an AWI `.workflow.js` interchange lane (manifest ↔ JS export ↔
  bare JS scan). No JS engine (R3), no `unsafe`, no FFI, no new
  scheduler (R7/R10 safe). All metadata stamps are documented with
  "byte-identical legacy rows" invariant — every optional feature
  produces the same wire shape as the pre-feature code when unset.

### r9 carry-over status (from `workflow-2026-08-29.md`)

| r9 finding | Status at r10 | Evidence |
|---|---|---|
| **L** "subject format with raw colons at `compile.rs:498`" | **FIXED** | `compile.rs:704` — `subject: format!("{}:{}", sanitise_name(&def.name), sanitise_name(&step.id))`. Explicit comment: "Sanitise both parts so a `my:workflow` name or `step:1` id can't produce a subject with multiple colons". `sanitise_name` lives in `crate::json_canvas_io`. |
| **L** "cancel_partial sequential at `compile.rs:566-579`" | **FIXED** | `compile.rs:734` — anchor notified-stamp on `ids.iter().min()` first (writes the once-only `WORKFLOW_NOTIFIED_KEY` so the settle sweep sees "this run is settled" and does not double-notify), then `futures::future::join_all(ids.iter().map(...)).await` for the per-task `update_task` status writes. Import on `compile.rs:26`. The comment ("Concurrent status updates: the per-task write is independent… collapse to one wall-clock round-trip via `join_all`") documents the optimisation. |
| **M** `StepPins` body-iteration was a hard-coded list | **REFACTORED-AWAY** | `StepPins` is now a struct with exhaustive `census()` destructuring; `all_fields()` derives from a fully-populated value's census; `stamp()` is data-driven. Adding a new pin fails the compile. |
| **M** model/effort dual-write inconsistency | **REFACTORED-AWAY** | Both ride through the same `StepPins` struct → `stamp()` → `WORKFLOW_MODEL_KEY`/`WORKFLOW_EFFORT_KEY`. `workflow_model_override` and `workflow_effort_think_level` are the dual accessors. |
| **M** "module-wide logging only at `warn!`" | **PARTIAL** | Still mostly `trace` + `warn!`, but `clarify.rs` now `tracing::warn!` on malformed metadata, and `materialize`'s `cancel_partial` arm `warn!`s per-row failures with structured fields (`anchor`, `task`, `error`). |
| **L** import path JS-overreach | **REFACTORED-AWAY** | Import is now a three-path bounded parser (bare JSON / embedded lossless block / bare JS scan), with `MAX_IMPORT_BYTES = 16 MiB` (memory amplification cap), and every reader either recognises a static literal or abstains to `dropped`. The "R7/R10 abstention" pattern is now explicit and named in tests (`nonliteral_arg_does_not_capture_unrelated_string`, `dynamic_choices_abstains`, etc.). |
| **L** WorkflowDef round-trip drift | **FIXED** | `def.rs` enforces byte-identical legacy wire shape via `skip_serializing_if` on the same defaults; `manifest.rs` separates `from_def` (drops extras) from `with_core_from` (preserves extras by step id, otherwise `save` would strip pins silently). |

**Net r9 carry-over**: every previously-flagged issue is FIXED or
REFACTORED-AWAY. No new findings escalate a r9 item.

### NEW findings (not in previous review)

- 0 critical / 3 warning / 4 suggested test.

---

## Critical

**(none.)**

The module is in exemplary shape: documented R3/R7/R10 invariants are
respected (no JS engine, no new scheduler, prompt language is source of
truth), the byte-identical legacy invariant is enforced at every
metadata stamp site, real bugs between r9 and r10 are explicitly named
in test functions (the surface is the bug), and the import/export
interchange lane is round-tripped both via the embedded lossless block
(closed under itself) and via the bare JS scan (best-effort, with
disclosure).

---

## Warning

### **W-1** [category: Architecture] src/workflow/compile.rs — `materialize` is at the 8-arg ceiling

- **Description**: `materialize(def, inputs, team_id, store, clarify_ctx, pins, strategy, origin_session)`
  takes eight parameters. The docstring already acknowledges the
  count ("this signature is already at eight, and because the two are
  one fact — see `RunInputs`") — referring to the historical merge of
  `input` and `args`. The remaining six are independently optional,
  conceptually three groups (identity: `def`, `store`, `team_id`; run-time
  options: `clarify_ctx`, `pins`, `strategy`; run-attribution:
  `origin_session`, `inputs`). At 8 args the function has hit the
  idiomatic-Rust ceiling — every future override needs a parameter.
- **Evidence**: `src/workflow/compile.rs:502-512` (signature),
  docstring at `compile.rs:492-501`.
- **Suggested fix** (optional): introduce `MaterializeRequest<'a>`
  carrying `inputs`, `team_id`, `clarify_ctx`, `pins`, `strategy`,
  `origin_session`. Leave `def` and `store` outside (they are unique
  components). New options become a field on the request, never a
  positional arg. Not blocking — there are currently no extra pins
  in flight.
- **Jev severity check**: aggregate 2.5 / 4.0 → 0.2-0.4 (nitpick tier).
  Score against the full set of findings (see Jev call record). This
  finding carries the most weight of the three and edges into refactor
  suggestion, not blocking.

### **W-2** [category: Quality] src/workflow/compile.rs — dead `if`-with-comment in `compute_parallel_assignments`

- **Description**: Inside `compute_parallel_assignments` (lines
  ~395-410), the loop body contains:
  ```rust
  if !label_seen.insert(label.to_string()) {
      // already counted on a previous step; just bump the counter.
  }
  ```
  The block body is a comment. `label_seen: HashSet<String>` is
  inserted into but never read — the counter increment in
  `counters.entry(label.to_string()).or_insert(0)` is independent of
  the set's membership. The `if` is dead, the `HashSet` is dead, the
  `label_seen` line of the second pass is dead. `cargo clippy`'s
  `let_underscore_must_use` / `unused_assignments` would likely catch
  this if the module were linted at this strictness (Aleph
  compiles with `-D warnings`, but the lint set is conservative on
  side-effecting `HashSet::insert`).
- **Evidence**: `src/workflow/compile.rs:~398-405`. The full
  `compute_parallel_assignments` function body shows the dangling
  set.
- **Suggested fix**: delete the `if !label_seen.insert(label.to_string()) { /* … */ }`
  block and the `let mut label_seen: HashSet<String> = HashSet::new();`
  declaration above the first pass. Three lines removed, no
  behaviour change.
- **Jev severity check**: 2.5 / 4.0 → 0.0-0.2 (documented choice
  / nitpick tier). Dead code in a quality-reviewed module is
  informational; nothing executes wrongly.

### **W-3** [category: Quality] src/workflow/compile.rs — `parallel_groups_for` `unwrap_or_default()` with documented unreachable branch

- **Description**: `parallel_groups_for` uses
  `groups.remove(&label).unwrap_or_default()` at the end of the
  per-label mapping. The same source line of code carries an
  explicit 5-line comment that says "Unreachable in practice: the
  entry was inserted above. `unwrap` would be clearer than
  `unwrap_or_default()` (which silently hides a bug if the map ever
  gets mutated between the two loops), and the fallback is
  unreachable by construction."
- **Evidence**: `src/workflow/compile.rs:~340-352` (`parallel_groups_for`).
- **Suggested fix**: change to `.unwrap()` (or `.expect("label
  membership invariant — see .entry().or_default() above")`). The
  author has already done the thinking and written the rationale;
  the only remaining step is to match the implementation to the
  rationale.
- **Jev severity check**: 2.5 / 4.0 → 0.0-0.2 (documented design
  choice tier). The author's note is the right note; this is a
  one-character cleanup that closes the gap between comment and
  code.

---

## Suggested Test

### **T-1** [category: Quality] src/workflow/compile.rs — direct unit test for `cancel_partial`

- **Description**: `cancel_partial` is exercised through
  `materialize`'s `create_task` error path (which it isn't — there
  is no negative test of materialize's failure path either). Both
  branches — anchor stamp first then `join_all` of status writes —
  deserve a focused test: (1) anchor's `WORKFLOW_NOTIFIED_KEY` is
  set with `NOTIFIED_BY_CANCEL` provenance; (2) every task's status
  is `Cancelled`; (3) failure of the anchor stamp does not abort the
  status writes; (4) failure of one status write does not abort the
  others (best-effort, concurrent).
- **Evidence**: `src/workflow/compile.rs:723-779`. There are 22 tests
  in `compile.rs::tests`, none of them explicitly fails
  `create_task` mid-loop.
- **Suggested fix**: add a `cancel_partial_marks_anchor_first_then`
  test that constructs two tasks, points at a `MockCoordTaskStore`
  whose `create_task` returns Ok twice and where one of two
  pre-existing tasks rejects the status update — assert that the
  other task still flips to `Cancelled` (proves join_all is
  concurrent and best-effort).

### **T-2** [category: Quality] src/workflow/compile.rs — direct unit test for `compute_parallel_assignments`

- **Description**: The function has no direct test. It is exercised
  through `materialize` end-to-end (which the existing tests cover),
  but the counter logic is non-trivial (first-pass size, second-pass
  index per label) and would benefit from a unit test that does not
  need a `coord_task` store. A single 8-line test that calls
  `compute_parallel_assignments(&def)` and compares the map would do.
- **Suggested**: add `parallel_steps_indexes_correctly_labeled`,
  `parallel_steps_zero_based_per_label`, `parallel_steps_size_stable`.

### **T-3** [category: Quality] src/workflow/interop/import/mod.rs — `MAX_IMPORT_BYTES` enforcement

- **Description**: `parse_workflow_js` reads `MAX_IMPORT_BYTES = 16 MiB`
  up front; the function rejects oversized input before the
  memory-amplifying scan runs. There is no test that asserts this
  cap actually triggers.
- **Evidence**: `src/workflow/interop/import/mod.rs:~50-90` (cap
  constant + rejection).
- **Suggested fix**: add `parse_workflow_js_rejects_oversized_input`
  that feeds a `vec![b'x'; MAX_IMPORT_BYTES + 1]` and asserts the
  error message names the cap. The cap's correctness depends on the
  rejection firing before any parsing happens; this is the only test
  that proves it.

### **T-4** [category: Quality] src/workflow/proposal.rs — `accept` writes active before deleting draft

- **Description**: `accept(name)` does load → `store::save_at(active)` →
  `store::delete_at(draft)`. The current implementation logs and
  continues if `delete_at` fails (deliberate — do not fail on
  already-accepted). There is no test for this resilience, and no
  test that asserts the order is `save` before `delete` (a wrong
  order would lose the proposal on a crash).
- **Evidence**: `src/workflow/proposal.rs:~260-290` (`accept` body).
- **Suggested fix**: add `accept_writes_active_before_deleting_draft`
  using a fake store that fails delete on the draft dir — assert
  the active store has the workflow. Optionally assert failure of
  save_at does NOT call delete_at.

---

## Per-perspective findings

### Security

**(none.)**

The module handles three user-input surfaces:

1. **Workflow definitions** (`def.rs`): declarative, validated
   top-to-bottom. `validate()` rejects empty names, duplicate ids,
   unknown deps, self-deps, cycles, and per-kind invariant violations
   (agent name non-empty, clarify prompt non-empty, etc.).
   `scan_prompt` is **single-pass** — the comment is explicit: "Single-pass
   prevents `{{env}}` lookup value `{{x}}` from being re-expanded"
   (template-injection defense). Unsatisfied names are left as written
   rather than silently dropped. There is no second scan that could
   re-expand injected content.

2. **Persisted workflow files** (`store.rs`): file paths under
   `$ALEPH_HOME/workflows/*.json` are resolved through
   `sanitise_name` (`resolve_path_at`), which is the same function
   `compile.rs:704` uses for the task subject — path traversal is
   blocked by shared primitive. Atomic temp + rename (`{pid}.{seq}.tmp`)
   prevents partial-file reads. Last-writer-wins race is documented
   and the contract is "single-writer".  The `WorkflowListing`
   structure separates corrupt files by file (named problems, not
   hidden), so a hostile file cannot hide its existence.

3. **`.workflow.js` interchange** (`interop/`): no JS engine (R3).
   The parser is bounded — `parse_value` has a `MAX_DEPTH = 128`
   cap (mirrors serde_json's own cap), `parse_workflow_js` rejects
   `> MAX_IMPORT_BYTES = 16 MiB` up front (memory amplification
   guard ~4×), and every reader either recognises a *static literal*
   or abstains to `dropped` (R7/R10 honest-by-construction).
   `strip_string_literals` blanks prompt bodies before keyword
   needles run, so a prompt containing the literal text `pipeline(`
   cannot trip the imperative-construct detector. `blank_comments`
   blanks `//` and `/* */` comments before the bare scan runs, so a
   prompt with `//` cannot open a phantom string and swallow the
   rest of the file (the real bug that this code fixes has an
   explicit test name). Spreads (`...X`) in opts are detected and
   recorded in `opts_abandoned` rather than silently producing a
   half-captured step.

No secrets handling exists in the module — there are no tokens,
no credentials, no HMAC keys. The metadata channel carries strings
only (no binary), and the stamps are all visible in the source.

### Logic

**(none.)**

The state machine of `materialize` is small, complete, and well-
defended:

- **Validation** before iteration: `def.validate()?` enforces
  DAG-shape, then `def.topo_order()?` enforces acyclicity, then the
  iteration runs in topo order so a dependency is always
  materialised first. The "internal: dependency not yet
  materialised" branch is **unreachable by construction** — a
  topo-order iteration cannot reach a dependency before its
  dependee. The branch exists because removing it would couple the
  caller's invariant to the iteration protocol.
- **Idempotency / dedup**: `blocked_by` deduplicates step-level
  duplicate `depends_on` (otherwise the dependency table's
  `PRIMARY KEY` would abort `create_task`). The comment is
  explicit: "`validate()` permits duplicate `depends_on`
  (semantically a no-op), so collapse them here."
- **Concurrency**: `cancel_partial` is best-effort and concurrent
  (anchor first, `join_all` for the rest). `WorkflowRunBudget` uses
  `AtomicU64` with a CAS loop and `Ordering::Relaxed`, documented
  as "the right call (single-writer settle hook)". 100-thread ×
  50-step concurrent stress test confirms the boundary.
- **Cancellation semantics**: the `WORKFLOW_NOTIFIED_KEY` /
  `WORKFLOW_NOTIFIED_BY_KEY` provenance is the result of a
  documented design decision: provenance answers the question
  "was the marked run settled at stamp time?" that the age grace was
  trying to guess. The comment in `compile.rs:88-101` is one of the
  clearest in the module.
- **Determinism**: `audit_step_prompt` runs three keyword families
  in scan order (NOT a `BTreeMap`, because `BTreeMap` would re-sort
  and lose the position signal). Tests cover UTF-8 byte positions,
  malformed `{random:}` is skipped, no `{` open is no crash.
- **Determinism of the parallel-group computation**:
  `parallel_groups_for` uses `label_order: Vec<String>` to track
  declaration order so the outer `Vec<Vec<String>>` is stable across
  runs (otherwise the projection is a stable surface for tests and
  UI). `compute_parallel_assignments` is two-pass (size first, then
  index per label), so all members of a group see the same size.
- **Determinism of topology**: `topo_order` (Kahn's) preserves list
  order for roots. Tests cover `topo_order deps-before-underscore`.
- **Prompt expansion**: `render_prompt` calls `scan_prompt` with
  both `{input}` and `{{var}}` in one pass, preventing the
  template-injection re-scan. Unsatisfied `{{name}}` is left
  written (the audit runs separately; this is correct).
- **Race caveats**: `save_at` documents last-writer-wins race on
  the rename (single-writer contract); `unique_tmp_path` uses
  `{pid}.{seq}.tmp` so two concurrent writes do not collide on the
  temp name.

### Architecture

The r9 review called out a number of architectural concerns that
have all been refactored away. The current state:

- **No new scheduler**: `materialize` compiles into existing
  `coord_tasks`. The DAG scheduler is the dispatcher. R7 / R10 safe.
- **No JS engine**: the `.workflow.js` parser is hand-rolled and
  bounded. R3 safe.
- **No `unsafe`, no FFI**, no platform API. Core purity preserved.
- **Type-system boundary between layers**:
  - `WorkflowDef` (lean, declarative, wire-format-stable) ↔
    `WorkflowManifest` (rich, interchange-only with `phase` /
    `schema` / `isolation` / `agent_type`) ↔
    `StepPins` (the per-step executable overrides). Each carries
    what its lane carries and is validated at its boundary.
  - `WorkflowStepKind` (Agent default + skip-if-agent) +
    `CollectReduce` (Concat default) are byte-identical on the
    legacy wire. New fields go through `skip_serializing_if`.
- **Public surface in `mod.rs`**: 47 lines, all re-exports, no
  hidden types. The `StepPins::census` exhaustive destructuring is
  the architectural seam that made `effort` shipping visible (the
  comment calls this out by name).
- **Send/Sync**: no `Rc`/`RefCell` in this module; every shared
  piece goes through `serde_json::Value` or owned types. The
  `MaterializedWorkflow` is plain data.
- **Single-writer concurrency assumptions** are named where they
  exist (`save_at` last-writer-wins on rename; `WorkflowRunBudget`
  Relaxed on single-writer settle hook).

The one architectural nit is **W-1**: 8-arg `materialize` at the
idiomatic ceiling. Not blocking.

### Quality

The module is in the top tier of Aleph codebases for good quality.
Findings:

- **W-2** (dead `if`-with-comment block in
  `compute_parallel_assignments`) — three lines removed, no
  behaviour change.
- **W-3** (`unwrap_or_default()` where the comment explicitly
  prefers `unwrap()`) — one character fix, closes the gap between
  comment and code.

Both are the kind of finding that an in-flight cleanup pass should
address; neither is a behaviour bug.

What is exemplary:

- **Real-bug-fix tests are explicitly named**. The reader can grep
  `bare_js_parallel_block_reconstructs_siblings` and find the bug
  the test fixed. This is unusual and praise-worthy.
- **Comments document trade-offs, not behaviour**. E.g. "We are
  already on the error path of `materialise` and these are
  best-effort cancellations, so concurrent execution costs nothing
  and lets the dispatcher observe a fully-cancelled partial run in
  a single window instead of a sweep that may straddle a follow-up
  user prompt."
- **Byte-identical legacy invariant** is named at every site
  (14 metadata-key constants in `compile.rs` each carry this
  sentence in the docstring).
- **`deny_unknown_fields`** at the manifest top level (steps half
  already had it). Forward-compatibility is opt-in via known keys,
  not opt-out via unknown keys.
- **`StepPins::census` exhaustive destructuring** — adding a field
  is a compile error, forcing the author to answer "does this
  need a surface?" at the moment of addition. The r9 finding
  ("`effort` shipped invisible") is now structurally impossible.
- **Two-pass with size-first** in `compute_parallel_assignments` —
  the comment explains why one-pass is wrong ("two stampers could
  disagree on the group's size if a step gets added between
  passes").
- **No magic numbers** in the executable path. `MAX_DEPTH = 128`
  (cited), `MAX_IMPORT_BYTES = 16 MiB` (cited), `MAX_NAME_LEN = 80`
  (cited), `ABANDON_SNIPPET_CHARS = 60` (cited).
- **Test naming** uses snake_case and documents the input/output in
  the name (e.g. `bare_js_imports_join_array_prompt`,
  `bare_js_resolves_hoisted_schema_const`). A reader can grep
  behaviour by test name without reading the body.

---

## Conclusion

### Net delta from r9

- **All r9 Medium/Low items are FIXED or REFACTORED-AWAY**.
  Concretely:
  - subject format raw colons — **FIXED**
    (`compile.rs:704`, `sanitise_name` both parts)
  - `cancel_partial` sequential — **FIXED**
    (`compile.rs:734`, `join_all` after anchor stamp)
  - `StepPins` hard-coded list — **REFACTORED-AWAY** (struct +
    `census()` exhaustive)
  - model/effort dual-write inconsistency — **REFACTORED-AWAY**
    (both through `StepPins::stamp`)
  - import path JS-overreach — **REFACTORED-AWAY** (bounded three-
    path parser, explicit `dropped` honesty)
  - WorkflowDef round-trip drift — **FIXED**
    (`skip_serializing_if` defaults + `manifest.rs` extras preserved
    by id)

- **No r9 item has escalated**.
- **No new Critical / Warning findings block merge.** The 3 new
  warnings are nitpick / documented-choice tier (Jev aggregate 2.5
  / 4.0).

### Fix order proposal (this module only)

If a maintainer wants to address the findings:

1. **W-2** (delete dead `if`-with-comment in
   `compute_parallel_assignments`) — three lines, no behaviour
   change. Trivial.
2. **W-3** (`unwrap_or_default()` → `unwrap()` in
   `parallel_groups_for`) — one character. The comment already
   says to do this.
3. **T-1 / T-2 / T-3 / T-4** — direct unit tests for
   `cancel_partial`, `compute_parallel_assignments`,
   `MAX_IMPORT_BYTES`, `accept`'s save-before-delete order. Pure
   test coverage work.
4. **W-1** (8-arg `materialize`) — `MaterializeRequest<'a>` struct
   refactor. Only if a new override is being added; if not, leave
   it.

None of them is blocking. The module is in good shape for merge.

### Conclusion line

**shape-up.**