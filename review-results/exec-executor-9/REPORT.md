# Review Report — Batch 9: `src/executor/builtin_registry/definitions.rs` + `src/executor/builtin_registry/builder/{mod.rs,tests.rs}`

**Date:** 2026-08-11
**Scope:** `src/executor/builtin_registry/definitions.rs` (1789 lines) +
`src/executor/builtin_registry/builder/mod.rs` (13 lines) +
`src/executor/builtin_registry/builder/tests.rs` (289 lines) — 2091 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     3 |   2 |    7 |

This batch is the **catalog + tests** surface. `definitions.rs` is
the static, unconditional subset of the tool surface — the names
and descriptions the model sees regardless of runtime configuration.
`builder/tests.rs` is the integration test for the registry
construction. The findings here are about consistency between the
catalog, the registry's runtime registration, and the dispatch
table.

## Findings

### [HIGH] `definitions.rs:no_catalog_entry_inlines_its_description` (line 1430-1470) — source-scan shape is fragile to rustfmt changes
**Category:** quality
**Confidence:** High

**Description.** The test at line 1430-1470 scans the source file
character-by-character to find every `description:` site, expecting
each to be a single statement. The scan accumulates from
`description: ` to the terminating `,`, then asserts the
accumulated statement contains `DESCRIPTION`.

The HIGH is that the scan shape is "exact match per line shape":
- A multi-line `description:` that wraps over two lines (e.g.
  when the path is long and rustfmt wraps it) would have the
  `,` on the second line, which the scan does catch (it
  accumulates until `,`). OK.
- BUT: a `description: "literal text",` form (which the
  invariant is supposed to catch) is **not** tested by the
  shape. The scan would see `description: "literal text",` as
  one site, and the assertion `!site.contains("DESCRIPTION")`
  catches it. OK.
- The real fragility: the scan counts sites and asserts the
  count matches `BUILTIN_TOOL_DEFINITIONS.len()`. If a future
  refactor adds a `description:` field OUTSIDE the catalog
  (e.g. a private helper struct), the count drifts and the
  test fails. The fix is to scope the scan to a known region
  (e.g. between `pub const BUILTIN_TOOL_DEFINITIONS: &[…]` and
  the closing `];`).

**Suggested fix.** Scope the scan to the catalog body:

```rust
let src = include_str!("definitions.rs");
let start = src.find("pub const BUILTIN_TOOL_DEFINITIONS")
    .expect("catalog constant present");
let end = src[start..].find("];")
    .map(|i| start + i)
    .expect("catalog terminator present");
let catalog_src = &src[start..=end];
```

Then scan `catalog_src` instead of `src`. The count-vs-catalog
invariant is now a structural property of the catalog body, not
of the whole file.

### [HIGH] `definitions.rs:CATALOG_DESCRIPTION_CEILING_BYTES` (line 1483-1530) — ceiling is hand-tuned, not measured
**Category:** architecture
**Confidence:** High

**Description.** The ceiling constant is `94_306` bytes, hand-tuned
across multiple rounds documented in the long comment block
(2026-08-04, 2026-08-05, 2026-08-06, 2026-08-10). The test
`catalog_description_bytes_ratchet` asserts the total is below
the ceiling.

The HIGH is that the ceiling is **a number, not a measurement**.
The rounds have been correct, but the process is "raise the
constant by the new total" rather than "compute a target and
hold it". A future contributor who wants to add a 5 KB tool
description would need to either (a) raise the ceiling by 5 KB
or (b) shrink another tool's description by 5 KB. The constant
drifts upward; the budget does not.

**Suggested fix.** Compute the ceiling as
`max(measured, floor) + headroom` so the constant is a
**derived** number, not a hand-set one. The test then asserts
"below the derived ceiling" rather than "below N bytes":

```rust
const HEADROOM_BYTES: usize = 1024;
const FLOOR_BYTES: usize = 80_000;
let ceiling = (measured_total + HEADROOM_BYTES).max(FLOOR_BYTES);
```

This is a refactor, not a fix. For this pass, no change.

### [MEDIUM] `definitions.rs:every_registered_core_tool_is_accounted` (line 1537-1620) — CRLF-safe scan but not indent-safe
**Category:** quality
**Confidence:** High

**Description.** The test at line 1544 strips `\r` (CRLF-safe) but
the scan is shape-sensitive: it expects every `reg(` opener to
be on its own line, followed by `tools,` on the next, followed
by a string literal. A future `reg(...)` call that puts the
opener and the args on one line (or uses a different helper
signature) silently bypasses the test.

The test handles the scan failure with `assert_eq!(registered.len(),
openers, "...")` to catch a count drift, which is the right
shape. But a `reg` helper rename or signature change would
silently miss sites without the count check noticing (because
both `openers` and `registered` are zero).

**Suggested fix.** Add a minimum-opener count assertion:

```rust
assert!(openers > 20, "only {} `reg(` openers in core_tools.rs \
                       — either the scan shape is wrong or the \
                       file was gutted", openers);
```

This is already in the test (line 1582-1587). Good. The MEDIUM
is the **scanner robustness to whitespace**: any future
`reg    (` (with extra whitespace) breaks the scan. Tighten the
matcher to use regex or to strip all leading whitespace.

### [MEDIUM] `definitions.rs:test_all_tools_defined` (line 1700-1740) — list of assertions is hand-maintained
**Category:** quality
**Confidence:** High

**Description.** The test asserts a specific list of tool names
exists in `BUILTIN_TOOL_DEFINITIONS`. The list is
hand-maintained and grows as new tools are added. A new tool
that is added to the catalog is automatically picked up by the
`contains` check, so the test does not need an update. The list
is a guard against silent removals.

The MEDIUM is that the assertions are duplicated against the
groups test (`groups.rs::test_all_builtin_tools_have_a_group`).
A tool that is removed from `BUILTIN_TOOL_DEFINITIONS` but still
in `TOOL_CATEGORIES` would fail the groups test (forward
direction) but NOT this test (this test only asserts the names
are present, not that they are absent).

**Suggested fix.** No change — the two tests are complementary
(forward and reverse of different tables). The hand-maintained
list is small and the cost of an out-of-date name is just a
"Tool X not in list" failure at CI.

### [MEDIUM] `definitions.rs:test_create_tool_boxed` (line 1790-1830) — covers "no config" half but not "config present" half
**Category:** quality
**Confidence:** High

**Description.** The test asserts
`create_tool_boxed("unknown", None).is_none()` and
`create_tool_boxed("image_generate", None).is_none()` (the
"no config" path). It does NOT assert the "config present"
path: `create_tool_boxed("image_generate", &config)` where
`config.generation_registry.is_some()`. A future change to the
"config present" arm would compile and pass every test.

**Suggested fix.** Add a `with_config_present` test path:

```rust
#[test]
fn test_create_tool_boxed_with_generation_registry() {
    let cfg = BuiltinToolConfig {
        generation_registry: Some(Arc::new(RwLock::new(GenerationProviderRegistry::default()))),
        ..Default::default()
    };
    // image_generate requires a generation registry, but with the
    // empty default registry the first_for_type check is also empty.
    // The current behaviour is None for empty registry. A test that
    // exercises a non-empty registry is the right assertion.
    let _ = create_tool_boxed("image_generate", Some(&cfg));
}
```

The MEDIUM is the gap, not the specific shape. A test that
exercises a populated config catches silent regressions in the
"with config" arms of `create_tool_boxed`.

### [LOW] `builder/tests.rs` — `IsolatedAlephHome` guard is in some tests, missing in others
**Category:** quality
**Confidence:** High

**Description.** The 4 spec3_tool_gating_tests + 2 agent_info_wiring_tests
+ 4 workspace_manage_wiring_tests use `IsolatedAlephHome::new()`.
The 5 sessions_tests at `registry/tests.rs` (Batch 6's purview) also
use it. The 3 plugin-handler tests in `mod tests` (Batch 6) do not
use it because they call `BuiltinToolRegistry::new()` via
`Default::default()` and don't touch the data dir.

The pattern is correct (tests that touch the goal store / data dir
need the guard; tests that don't, don't). The LOW is the
**duplication** of the `let _home = …` boilerplate across 10+
tests. A `TestRegistry` helper that wraps the guard + the
construction would close the duplication.

**Suggested fix.** Out of scope. The pattern is documented in
the registry/tests.rs file's comment at line 22-30.

### [LOW] `builder/mod.rs` — re-exports `BuiltinToolConfig` and `BuiltinToolRegistry` but not the constructor submodules
**Category:** architecture
**Confidence:** Low

**Description.** `builder/mod.rs` has `pub use crate::executor::builtin_registry::{BuiltinToolConfig, BuiltinToolRegistry};`
(line 5-7) but does not re-export the `register_core_tools`,
`register_optional_tools`, `build_agent_acp_a2a_tools`,
`build_coord_team_tools`, `build_collab_session_tools` helpers.
These are `pub(crate)` and only callable from the same crate.

**Suggested fix.** No change — the helpers are crate-internal by
design. The `mod.rs` is the seam; the helpers are
implementation details.

## Cross-References

- `definitions.rs:BUILTIN_TOOL_DEFINITIONS` (line 70-1000) — the
  static, unconditional subset of the tool surface. The
  invariant `no_catalog_entry_inlines_its_description` is the
  source-level guard; the invariant `every_registered_core_tool_is_accounted`
  is the cross-table guard.
- `definitions.rs:create_tool_boxed` (line 1020-1300) — the
  legacy construction path. The new `BuiltinToolRegistry::with_config`
  is the runtime path; `create_tool_boxed` is the slash-command
  and AlephToolServer path. The two must agree on which tools
  exist unconditionally.
- `builder/tests.rs::spec3_tool_gating_tests` (line 1-150) — the
  Context / Tools / Hybrid mode gate. The 6+ tools that
  `expose_retrieval_tools` gates are listed in `MEMORY_RETRIEVAL_TOOLS`
  at line 13-22. A tool that joins this list (e.g. a future
  `memory_diff`) must be added there.
- `builder/tests.rs::workspace_manage_wiring_tests` (line 200-289) —
  the 5-registration chain test for `workspace_manage`. The
  `agent_info_wiring_tests` (line 150-200) is the same shape for
  `agent_info`. Both guard against the "advertised but
  undispatchable" failure mode.
