# Task 28 Independent SPEC + QUALITY Review

**Review scope:** `.` five-file Task28 repair only. Read-only review/verification; the only file written by this review is `task-28-review.md`. No source, Cargo, QA, test, runner, stage, commit, reset, checkout, install, graph, or nested-agent action was performed.

## Verdicts

- **SPEC COMPLIANCE: PASS.** The actual five-file diff matches the approved bounded repair. The source gate passed before Cargo execution. All five required guarded suites passed in the required serial order, with fresh memory probes, `jobs=2`, `timeout_s=900`, no timeout, and no surviving descendants.
- **TASK QUALITY: PASS WITH LOW CONCERNS.** Behavior and coverage are verified. The only findings are misleading test comments and two unused error payload warnings in the new projection fixture; neither changes tested behavior or violates the repair contract.

## Source gate and scope findings

### Approved source changes

- `src/a2a/adapter/server/routes.rs:327-344,459-565`: `extract_credentials` uses `split_once(' ')` and `eq_ignore_ascii_case("Bearer")`; token bytes, empty remainder, extra internal spaces, fallback, and precedence are preserved. The bounded table includes mixed schemes, token preservation, empty/extra-space cases, near-miss scheme/prefix/leading-space/tab cases, and fallback/precedence. No authentication verification or authorization grant was added.
- `src/browser/cdp_backend/mod.rs:1240-1253`: the real `recording_test_encoder.rs` is included in the 15-entry `include_str!` census. The actual directory comparison, parser, placeholder mutant arms, marker assertions, and nonzero census remain present (`:1267` and adjacent test body).
- `src/capability/projection_host.rs:1634-1710,2817-2899`: the private `BlockingHandler`, real `ScopedToolService`/canonical registry composition, real `McpRegistryTool`, call identity, handler-entered signal, captured/fresh `ResolveError::Revoked`, second-call pre-handler rejection, first-call completion, and bounded Tokio timeouts are all present. The fixture uses the same registry store and host facade; it does not mint admission tokens, unwrap a raw handler for invocation, or claim `ScopedToolService` independently checks ownership.
- `src/cluster/node_file_cmd.rs:355-449`: missing-file error is derived from a real `File::open` and asserts `NotFound` plus exact `file.read: {io_error}`; `MAX_FILE_BYTES + 1` asserts the exact cap error; exactly `MAX_FILE_BYTES` asserts successful size, hash, and byte-for-byte decoded content.
- `src/config/tests/validation.rs:178-308`: the private scoped warning layer captures the real warning; the original `command(".*", "nonexistent", None)` and four malformed patterns remain; retired enumeration, boot success, and exact warning text are asserted. No routing/rejection restoration or production `validate.rs` change was made.

### Production-preservation check

The actual `git diff HEAD` was inspected. Files 2–5 have changes only in `cfg(test)` content (the validation test file is itself test-only). `src/a2a/adapter/server/routes.rs` has only the authorized `extract_credentials` implementation/comment change plus tests. No unrelated production hunk or scope leak was found in the reviewed diff.

### Low-severity findings

1. **LOW — inaccurate explanatory comment, not behavior:** `src/a2a/adapter/server/routes.rs:543-546` says the previous logic accidentally prevented `x-api-key` fallback for a non-Bearer authorization header. The old implementation also fell through to the `x-api-key` branch when both Bearer prefixes failed. The assertion is correct; only the rationale should be corrected in a later cleanup.
2. **LOW — inaccurate explanatory comment, not behavior:** `src/config/tests/validation.rs:211-213` says the retired arm fires before an “unknown-provider arm” in `validate_rules`; the inspected current `src/config/validate.rs:170-201` has no such arm. The test’s preserved fixture and `validate().is_ok()` behavior are correct.
3. **LOW — test-only warning:** `src/capability/projection_host.rs:1667-1668` reports the `ResolveError` and `ToolError` payload fields of `ProjectionCallError` as never read. This is harmless for the passing fixture but should be cleaned up if warning-free test builds are required.

No high/medium correctness, security, authorization, race, bounded-wait, or scope findings were found.

## Required guarded verification

All commands were foreground, serial, exact contract commands. Each runner performed its own fresh `python3 -B qa/browser_live/check_memory.py` probe and reported `probe_exit=0`, available bytes at least `4294967296`, `jobs=2`, and `timeout_s=900`. The complete stdout/stderr is retained at each log path below.

| Order | Label / exact filter | Full log | Probe | Runner/Cargo | Timeout / descendants | Actual named count |
|---|---|---|---|---|---|---|
| 1 | `task28-a2a`; `a2a::adapter::server::routes::tests::` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-131020-task28-a2a.log` | exit 0; `6256164864` bytes | exit 0 | `timed_out=False`; `descendants_remaining=0`; 331.2s | **16 passed, 0 failed**; includes `extract_credentials_bearer_table` and all retained route tests |
| 2 | `task28-cdp-census`; exact `browser::cdp_backend::tests::no_verb_is_left_unwired` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-131850-task28-cdp-census.log` | exit 0; `6837469184` bytes | exit 0 | `timed_out=False`; `descendants_remaining=0`; 1.4s | **1 passed, 0 failed**: `no_verb_is_left_unwired` |
| 3 | `task28-projection-inflight`; exact `capability::projection_host::tests::already_running_invocation_is_not_cut` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-131929-task28-projection-inflight.log` | exit 0; `6867238912` bytes | exit 0 | `timed_out=False`; `descendants_remaining=0`; 0.5s | **1 passed, 0 failed**: `already_running_invocation_is_not_cut` |
| 4 | `task28-node-file`; `cluster::node_file_cmd::tests::` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-132016-task28-node-file.log` | exit 0; `6945357824` bytes | exit 0 | `timed_out=False`; `descendants_remaining=0`; 1.3s | **8 passed, 0 failed**; includes `file_read_accepts_exact_cap_boundary`, `file_read_rejects_missing`, and `file_read_rejects_oversize` |
| 5 | `task28-retired-rules`; `config::tests::validation::` | `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-132056-task28-retired-rules.log` | exit 0; `6844792832` bytes | exit 0 | `timed_out=False`; `descendants_remaining=0`; 0.5s | **13 passed, 0 failed**; includes both renamed retired-rule tests |

The A2A run was not repeated: its existing complete log was read and accepted as the required first invocation. The prior interruption was an ACP watchdog failure, not a Cargo failure.

The runner stderr consistently contained 14 existing/general warnings, including the two test-only unused payload warnings identified above; no warning was promoted to an error and no test failed.

## Old-to-new coverage map

The seven historical Task6 failure cases are covered as follows:

| Historical failing test | Current coverage / result |
|---|---|
| `a2a::adapter::server::routes::tests::extract_credentials_bearer_mixed_case` | Same named test retained and passed; `extract_credentials_bearer_table` also passed |
| `browser::cdp_backend::tests::no_verb_is_left_unwired` | Same exact test retained, real 15-file census, passed |
| `capability::projection_host::tests::already_running_invocation_is_not_cut` | Same named test replaced with real in-flight composition, passed |
| `cluster::node_file_cmd::tests::file_read_rejects_missing` | Same named test retained with portable real `NotFound` assertion, passed |
| `cluster::node_file_cmd::tests::file_read_rejects_oversize` | Same named test retained with exact `8MiB` cap error, passed |
| `config::tests::validation::test_invalid_regex_patterns_fail` | Renamed to `malformed_keyword_rules_boot_and_are_listed_as_retired`, passed |
| `config::tests::validation::test_rule_referencing_unknown_provider_fails` | Renamed to `a_retired_keyword_rule_with_unknown_provider_boots_and_warns_exact_text`, passed |

Historical evidence remains in `.superpowers/sdd/2026-10-14-modularization-p0-implementation/task-6-core-seven-triage.md`; the historical seven-test exit-101 results were not treated as a baseline waiver.

## Hash and protection verification

Post-test checks matched the repair reports and protected baseline:

| Artifact | Current SHA256 / result |
|---|---|
| `src/a2a/adapter/server/routes.rs` | `feb5f8477a2920edae6ce5258ee016e7ac26f38f80970ae15f323bc225cbd35d` |
| `src/browser/cdp_backend/mod.rs` | `8004004cb92fdd756682d84491565aec5e38c9bc824ea9b638d7a57a3010643f` |
| `src/capability/projection_host.rs` | `f8006d31954ea40322a6dc9d000e5a8ca66772a9cc5e45fcd7f94f6df9ade57e` |
| `src/cluster/node_file_cmd.rs` | `f8607712a8fe517c6b89b16f4b31fa5285798039ca45e3dac37fb459d5e1e768` |
| `src/config/tests/validation.rs` | `5d1c0752aa06e23d51f48719db8b9a70e793692fc292d6d1ce4bda3acf82b214` |
| Protected 94-file aggregate | `c660ef437517249c61ff302bc0e7662f5daa7e73f3f533541739f2f8c970ad9e` — exact match |
| `Cargo.toml` | `a35e7af8192fb2bcc619ea6db4336124302ea39e02bee8e08c26815747390b94` — exact match |
| `Cargo.lock` | `070e502d63ef94fb806fcc63c9c0050e4726d7aa9f1a208b8eeeb4371c66803e` — exact match |
| `qa/browser_live/check_memory.py` | `85434b7c552efa28df22837156d0f5365ec09659f7f4a827d2623aaa8f160025` — exact match |
| `guarded-cargo.py` | `c8a4ff2f4f10d3867080e9239539008eeecab5a7a365aae3514e3cc20ab47cc6` — exact match |
| `tests/modularization_contracts.rs` | `9de48f6107ac0141e063b92d8db2844c085f793317b4a60a8f177e33ba29ce82` — exact match |
| Cached index binary hash | `eaaaaa3129a40f7b53eea2619f9cca26c02d2ac5c2812e0ba8c01c6e4af94705` — exact match |

Accepted Task6 consumer-import freeze also remains exact:

- `crates/aleph-slot-api/src/slot.rs`: `0c01c2153f6758b6e742e60736c72272713dcdef9b862b487b3cfa6a78d3612b`
- `crates/aleph-slot-api/src/hooks.rs`: `b0572f729dbfecf45076577804e98160e07a1614fc84f3d0a9603accee691498`
- `crates/aleph-slot-api/src/lib.rs`: `95bc61a5358e1b55ca955dac02f1d1cd0c448097a05705fcf268a04c9af7a528`
- `crates/aleph-slot-api/tests/hook_compat.rs`: `3804bf74ae6734702432799650507abf74e8cb1818115d35fd77da12a94d84be`

Direct post-test checks:

- `rustfmt --edition 2021 --config skip_children=true --check` on all five source files: **exit 0**, no output, no mutation.
- Scoped `git diff --check` on all five source files: **exit 0**, no output.

## Explicit limits / NOT DONE

- **NOT DONE:** no full Core test suite, workspace test suite, `cargo clippy`, cross-platform build/test, live QA, or HEAD runtime baseline.
- **NOT DONE:** no claim that this closes full P0 or P0–P6; this review closes only the bounded Task28 SPEC+QUALITY gap.
- The review did not modify the three low-severity comments or test-only warning payloads because the reviewer is read-only and the user authorized only `task-28-review.md` as writable.
- Existing unrelated working-tree modifications were preserved; this review did not reset, stage, commit, or normalize them.

**Final status: Task28 bounded review accepted — SPEC PASS; QUALITY PASS WITH LOW CONCERNS.**
