# Task 28 — Four-File Repair Report

> Temporary report. Required by the user instruction
> "完成后只做直接 rustfmt ... 及 git diff --check（允许），写 task-28-four-files-report.md".

## Scope (per task-28-repair-brief.md, sections 1, 2, 4, 5)

Four independent file repairs, all under `src/`:

| # | File | Subsystem | Brief section |
|---|------|-----------|---------------|
| 1 | `src/a2a/adapter/server/routes.rs` | A2A server | §1 — `extract_credentials` |
| 2 | `src/browser/cdp_backend/mod.rs` | CDP backend | §2 — `cfg(test)` census |
| 3 | `src/cluster/node_file_cmd.rs` | Cluster node file cmd | §4 — `cfg(test)` missing/oversize/boundary |
| 4 | `src/config/tests/validation.rs` | Config validation | §5 — renamed "retired rule boots" tests |

`src/capability/projection_host.rs` (Phase 3) is explicitly forbidden in this
task — left untouched.

## Actual file SHA256 (after edits, post-rustfmt)

| File | SHA256 | Lines |
|------|--------|-------|
| `src/a2a/adapter/server/routes.rs` | `feb5f8477a2920edae6ce5258ee016e7ac26f38f80970ae15f323bc225cbd35d` | 916 |
| `src/browser/cdp_backend/mod.rs` | `8004004cb92fdd756682d84491565aec5e38c9bc824ea9b638d7a57a3010643f` | 1869 |
| `src/cluster/node_file_cmd.rs` | `f8607712a8fe517c6b89b16f4b31fa5285798039ca45e3dac37fb459d5e1e768` | 437 |
| `src/config/tests/validation.rs` | `5d1c0752aa06e23d51f48719db8b9a70e793692fc292d6d1ce4bda3acf82b214` | 333 |

## Old test name → new test name map

### File 1 — `src/a2a/adapter/server/routes.rs` (A2A `extract_credentials`)

Brief §1: replace function so that scheme is
`eq_ignore_ascii_case("Bearer")` over a single ASCII space separator, the
token is NOT trimmed/lowered, empty remainder and extra-whitespace cases are
preserved, and `x-api-key` fallback/precedence is retained. Add a finite
table test covering mixed scheme/token preservation, near-miss, whitespace,
and fallback.

| Concern | Status |
|---------|--------|
| `extract_credentials` (production fn) | REWRITTEN at `src/a2a/adapter/server/routes.rs:336` — the new body is `eq_ignore_ascii_case("Bearer")` over a single ASCII space, no trim, no lower, empty remainder kept, extra whitespace kept, `x-api-key` fallback/precedence preserved. |
| Existing per-case tests | UNCHANGED — `extract_credentials_bearer_token`, `extract_credentials_bearer_lowercase`, `extract_credentials_bearer_mixed_case`, `extract_credentials_api_key`, `extract_credentials_none`, `extract_credentials_bearer_takes_precedence` at lines 398–456. |
| New finite table test | ADDED — `extract_credentials_bearer_table` at `src/a2a/adapter/server/routes.rs:459`. Covers: mixed-case scheme/token preservation, near-miss (`bearer`+`token`, `Token token`, `Bearer` missing space, `Bearer  token` two spaces, `Bearer\t` tab), x-api-key precedence, empty Authorization. |

### File 2 — `src/browser/cdp_backend/mod.rs` (CDP census)

Brief §2: `cfg(test)` census must include the real `recording_test_encoder.rs`,
keep the real directory comparison, keep the placeholder mutant arms and
marker assertions; do not touch production.

| Concern | Status |
|---------|--------|
| `include_str!` list (census) | UPDATED — `("recording_test_encoder.rs", include_str!("recording_test_encoder.rs"))` is now an explicit entry in `no_verb_is_left_unwired` at `src/browser/cdp_backend/mod.rs:1253`. The literal-assembly pattern (`"recording_test"` + `"_encoder.rs"`) is replaced by an explicit tuple; per-file `include_str!` literals require this. |
| Directory comparison | UNCHANGED — `read_dir(...).join("src/browser/cdp_backend")` block at the same site, asserting the listed scan list matches on-disk `.rs` files. |
| Placeholder mutant arms | UNCHANGED — `unwired` closure, `marker`/`helper` const concat, `no_verb_is_left_unwired` body all preserved. |
| Marker assertions | UNCHANGED — `assert!(unwired.is_empty(), ...)` and `scanned.len() == 15` invariant left intact. |
| Production code | UNCHANGED — no item outside `#[cfg(test)]` was edited. |

### File 3 — `src/cluster/node_file_cmd.rs` (Node file cmd tests)

Brief §4: only `cfg(test)` changes. `missing` test must use a real
`File::open`-derived `io::ErrorKind::NotFound` and assert against the
`file.read: {io}` error string. `oversize` keeps `MAX+1` and asserts the
exact `file.read: >{MAX_FILE_BYTES} bytes (cap enforced at read)`. New
test: `MAX` exactly succeeds, validates `size` and `content`. Production
`run` / resolver are not touched.

| Concern | Status |
|---------|--------|
| `file_read_rejects_missing` | CHANGED in body at `src/cluster/node_file_cmd.rs:355` — the test now derives the expected error from a real synchronous `std::fs::File::open(&target).expect_err("precondition: the file is absent")` (NOT `tokio::fs::File::open(...).await` as previously reported — there is no `.await` in the new code), asserts `io_err.kind() == std::io::ErrorKind::NotFound` on the derived error first, and then asserts the command's error is exactly `assert_eq!(err, format!("file.read: {io_err}"))` instead of the previous loose `assert!(err.contains("not found"), ...)`. The `tempdir` + `reader.run(json!({"path": "nope.bin"}))` skeleton is preserved. |
| `file_read_rejects_oversize` | CHANGED in body at `src/cluster/node_file_cmd.rs:383` — still writes `MAX_FILE_BYTES + 1` bytes (the write path is UNCHANGED), but the assertion is now exact `assert_eq!(err, format!("file.read: >{MAX_FILE_BYTES} bytes (cap enforced at read)"))` instead of the previous loose `assert!(err.contains("exceeds"), ...)`. |
| New `file_read_accepts_exact_cap_boundary` | ADDED at `src/cluster/node_file_cmd.rs:415`. Writes exactly `MAX_FILE_BYTES` bytes, asserts `file_read` returns `Ok(Value)` and the response contains `size == MAX_FILE_BYTES` and the original content. |
| Production `run`/`resolver` | UNCHANGED — only items inside `mod tests` (the `#[cfg(test)]` block at line 258) were touched. |

### File 4 — `src/config/tests/validation.rs` (Config retired-rule tests)

Brief §5: rename two misleading tests to "retired rule boots" semantics.
Assert `retired_keyword_rules` contains target position/regex, validate
returns `Ok`. Use existing `tracing` capability to capture real scoped
warnings, assert exact warning text. Do not modify `validate.rs`, do not
restore the old reject/routing behaviour. If capture interface is
uncertain, use a test-local `MakeWriter` + `tracing_subscriber::fmt::Subscriber`
+ `with_default`, NOT a global permanent subscriber.

| Old test name | New test name | Notes |
|---------------|---------------|-------|
| `test_rule_referencing_unknown_provider_fails` | `a_retired_keyword_rule_with_unknown_provider_boots_and_warns_exact_text` | Original fixture `command(".*", "nonexistent", None)` preserved verbatim per brief §5 (only the test semantics change from reject to retired warning + boot; the fixture must not change). The rule's regex `.*` is not `^/`-prefixed, so the retired-detection arm classifies it as a keyword rule and fires BEFORE the unknown-provider arm in `validate_rules`; the daemon still boots, the rule is listed in `retired_keyword_rules()` at position 1, and a captured `tracing` warn matches the exact retired-rule text. Asserts `retired_keyword_rules() == vec![(1, ".*")]`, `validate().is_ok()`, and captures the exact warning via a test-local `CapturedWarnMessages` `Layer<S>` + `set_default` (thread-local, scoped to this test) so the test does NOT install a global subscriber. |
| `test_invalid_regex_patterns_fail` | `malformed_keyword_rules_boot_and_are_listed_as_retired` | Same pattern. Asserts each of the four malformed patterns is at its 1-based position in `retired_keyword_rules()`, `validate().is_ok()`, and the captured warn count equals `patterns.len()`. |
| `CapturedWarnMessages` (test-local infrastructure) | NEW | `Arc<Mutex<Vec<String>>>` + `tracing_subscriber::Layer<S>` impl that visits the `message` field on every event. Dropped on test exit. No global subscriber. |
| `validate.rs` | UNCHANGED | No production file was modified. The two warn arms (real retire, malformed) remain where they were. |
| `reject`/`routing` behaviour | NOT RESTORED | The old `expect_invalid(..., "invalid regex")` and `expect_invalid(..., "unknown provider")` are gone, deliberately, because the brief is explicit that `Config::validate` never compiled regex bytes and the retired arm fires before the unknown-provider arm, so the original assertions were false greens. |

## Rustfmt + git diff --check results

| Command | Result |
|---------|--------|
| `rustfmt --edition 2021 --config skip_children=true src/a2a/adapter/server/routes.rs src/browser/cdp_backend/mod.rs src/cluster/node_file_cmd.rs src/config/tests/validation.rs` | exit 0, no output (rustfmt made any necessary adjustments in-place on the first run; subsequent runs are no-ops) |
| `git diff --check` (whole tree) | exit 0, no whitespace/conflict warnings |
| `git diff --check` (the four files only) | exit 0, no warnings |

## NOT TESTED AFTER REPAIR

Per the user instruction "不运行 Cargo/rustc":

- **NOT TESTED AFTER REPAIR**: no `cargo check`, `cargo test`, `cargo clippy`, or
  `cargo fmt --check` was executed. The four files were validated only by
  `rustfmt --edition 2021 --config skip_children=true` (per the user-permitted
  command) and `git diff --check`.
- **NOT RUN**: this report was produced by a single delegated worker (the main
  agent did not write the four edits in-line); no nested sub-agent was
  invoked from inside that worker, no background Cargo process was launched,
  no `git reset`/`install`/`graph`/`stage`/`commit` was executed.
- **NOT TOUCHED**: `src/capability/projection_host.rs` and every file outside
  the four-file list — no other source changes were made.

The new tests' *intent* matches brief §1, §2, §4, §5 to the letter, and
rustfmt and `git diff --check` are clean, but the user's explicit
"不运行 Cargo/rustc" rule means actual `cargo test` verification is left
to the next session.
