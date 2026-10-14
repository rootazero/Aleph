# Off-Loop + Managed + Gateway-Guard Verification

**Date**: 2026-10-12
**Worktree**: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`
**HEAD**: `34a26aece35553dca7d7a87e8d646ef74c6afd04` (dirty, 189 entries)
**Mode**: offline · `CARGO_BUILD_JOBS=1` · `CARGO_PROFILE_DEV_DEBUG=0` · codegen-units=256 · test-threads=1

## Scope of changes verified

Three independent work streams landed on disk before this verification:

1. **Off-loop RPC offload** (`a1ad7e21-ffc1-41c`):
   - New untracked file: `src/gateway/server/connection/off_loop.rs`
   - Modified: `src/gateway/server/connection/dispatch.rs` (re-reads live `CallerSnapshot` after lane permit for the allowlisted set), `src/gateway/server/connection/mod.rs` (queue/select arm + bounded shutdown), `src/gateway/lane.rs` (`extensions.installed` is `Query`).
2. **Gateway guard fixes** (`c1e27d4e-c6e0-495`):
   - `src/gateway/continuation_lifecycle.rs`: rename `close_session` → `close_browser_session`; the rename stops a false-positive over-keying.
   - `src/gateway/handlers/events.rs`: extend the `browser.live.*` carve-out so older readers see the carve-out as a plain subscription.
   - `src/gateway/caller_identity.rs`: add `caller_identity` to `AMBIENT_OWNER_CENSUS` so the census count matches the live call sites.
3. **Windows managed launcher (contract + narrow OS tests only)** (`dec6eed7-60b4-419`):
   - New untracked files: `src/sandbox/platforms/windows/elevation.rs`, `src/sandbox/platforms/windows/managed_launch.rs`, `src/sandbox/platforms/windows/wfp.rs`, `src/sandbox/windows_init/policy.rs`.
   - Modified: `src/sandbox/driver.rs`, `src/sandbox/platforms/windows/driver.rs`, `src/sandbox/policy.rs`, `src/sandbox/windows_init/imp/app_container.rs`, `src/sandbox/windows_init/imp/mod.rs`, `src/sandbox/windows_init/mod.rs`.
   - `CURRENT_CAPABILITIES` remains all `false` (runtime `UnsupportedPolicy`). No real admin/WFP/AppContainer, no production `ManagedLaunchApi`, no service-PID handoff, no capability flip.

Plus untracked browser-live new files used in the tests below:
- `src/browser/live/resource.rs`, `src/browser/live/lifecycle.rs`.

## Library compile

```
cargo check -p alephcore --lib --offline          →  Finished `dev` profile in 3m 31s  (no errors, no warnings)
cargo check -p alephcore --lib --tests --offline  →  Finished `dev` profile in 12m 43s (no errors, no warnings)
```

## Targeted test results

All test runs were `cargo test -p alephcore --lib --offline` with one `TESTNAME`, `--test-threads=1`, and `--no-fail-fast`.

| Module | Pass / Total | Notes |
|---|---:|---|
| `gateway::server::connection::off_loop` | **9 / 9** | Every off-loop guarantee holds: queue promotion, panic-safety, no head-of-line blocking, allowlist is exactly the audited set, saturation never runs inline, bounded shutdown, connection loop uses the off-loop set. |
| `gateway::server::connection` (full) | **28 / 28** | Includes 9 off-loop + 19 auth/verified-user tests. `auth::credential_provenance_tests`, `auth::revalidate_credential_tests`, `auth::verified_user_tests` all green. |
| `gateway::lane` | **21 / 21** | Lane permit + Query lane classification. `extensions_installed_is_query_but_its_write_twins_are_not` passes. |
| `gateway::continuation_lifecycle` | **6 / 6** | Side-session retirement, key roll, transcript/search-mirror wipe, no-op retirement, full-face coverage. The `close_browser_session` rename did not break any of these. |
| `gateway::handlers::events` | **9 / 9** | Carve-out withholds exactly the named topic; older servers read the carve-out as a plain subscription. |
| `gateway::visibility` | **34 / 34** | `every_ambient_owner_call_site_is_in_the_census` and `every_production_caller_of_ambient_actor_is_classified` both pass — the new `caller_identity` census entry is required. |
| `gateway::caller_identity` | **13 / 13** | Scope round-trips, verified-user requirements, legacy helper never carries/inherits a verified user. |
| `browser::live::resource` | **13 / 13** | Binding source/provenance immutable across revisions, `expired` emitted exactly once, refusal revokes grants in the same step, `close` emits once and tombstones, owner-scoped lookup is generic. |
| `browser::live::lifecycle` | **16 / 16** | Whole-batch revoke, sweep emits once, refusal closes, safe CAS keeps existing grant, terminal sink maps target destroyed/owner ended. |
| `extension::lifecycle` | **28 / 28** | Activation settled, mount/unmount, scope key, transition lock, hooks spelling, reload is a fixed point. (75s.) |
| `sandbox::platforms::windows::managed_launch` | **20 / 20** | 3 real OS native tests + 17 contract tests. Includes `suspended_identity_comes_from_the_owned_handle_and_never_runs_before_admission`, `unresumed_suspended_process_is_reaped_through_the_job_handle`, `terminate_on_a_process_outside_the_job_is_not_confirmed_empty`. `current_capabilities_stay_all_false_until_a_verified_impl_exists` holds. |
| `gateway::pty` (full) | 987 / **988** | **1 pre-existing failure**: `every_test_that_reaches_the_global_pty_manager_is_tagged`. See "Pre-existing issues" below. |
| `project_skill_block_none_when_no_project_skills` | **0 / 1** | **Pre-existing failure**. See "Pre-existing issues" below. |

## Pre-existing issues (not regressions)

Two failures were documented in the root-cause analysis (`07db625c-4422-497`) and are not introduced or fixed by any of the three work streams verified here. They are out of scope for the off-loop/managed/gateway-guard changes and would each require a separate, targeted fix.

1. **PTY global manager tag guard** — `gateway::handlers::pty::tests::every_test_that_reaches_the_global_pty_manager_is_tagged` asserts that every test calling `pty::manager()` must carry `#[serial_test::parallel(pty_global_manager)]`. The guard found 11 untagged call sites in `src/builtin_tools/terminal.rs`, `src/builtin_tools/terminal/tests.rs`, and `src/gateway/pty/runtime.rs` (the helper `manager()` is a shared helper reached outside `#[test]` bodies). Fix requires either moving the call into the test bodies or tagging every test in the affected files. **Untouched here.**

2. **`project_skill_block_none_when_no_project_skills`** — `gateway::execution_engine::run_loop::tests::project_skill_block_none_when_no_project_skills` asserts `collect_project_skill_block(tmp.path()).is_none()`. The assertion fails in the current tree. **Untouched here.**

## What this verification establishes

- The three work streams compile clean in `alephcore --lib --tests`.
- Every test authored by the three agents passes.
- No new failures were introduced (the two pre-existing failures are the same two documented in the root-cause analysis).
- The Windows managed-launcher contract holds: `CURRENT_CAPABILITIES` all `false`, `UnsupportedPolicy` at runtime, real OS tests confined to the three narrow shapes (suspended identity, Job admission/reap, no kill outside Job).

## What this verification does NOT establish

- It does not constitute a managed-companion / Native / full-chain E2E.
- It does not change the documented managed companion blockers (admin/WFP/AppContainer/service-PID handoff). The contract remains `UnsupportedPolicy`.
- It does not re-run the full workspace `cargo test` or `cargo clippy` on every crate. A full clippy pass is queued below.
- It does not prove the `pty` global manager guard or the `project_skill_block_none` test are solvable in this turn.

## Next verification steps

1. Run `cargo clippy -p alephcore --lib --no-deps --offline -- -D warnings` on the changed surface to confirm no new clippy lints.
2. Run a focused `cargo test` over `gateway::handlers::browser_viewer` to confirm the new `close_browser_session` rename in callers still works.
3. Update `b-plan-real-machine-addendum.md` and `task-23-acceptance-report.md` with the actual test counts.
