# Task 3 issue/deactivation race fix / 签发与停用竞态修复

## Scope and result / 范围与结果

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`.

The reported late-insertion race is fixed and covered by a deterministic real-dispatch regression. This is a narrow code/unit verification result, **not full Task 3 or Task 23 completion**, and not production/live Gateway verification.

已修复所报告的 sweep 后晚插入竞态，新增真实 dispatch 屏障回归通过。本报告仅证明该修复及定向回归，不宣称 Task 3 / Task 23 全部完成或真机验收。

## Changed files / 修改文件

These files already contained uncommitted Task 3/earlier work; the existing changes were retained.

- `src/browser/live/grant.rs`: an optional internal principal-validator callback lives in `Inner`. `set_principal_validator` installs it; `finish_issue` validates issuer and bound principal **while holding the entries mutex**, before pruning/eviction/insertion. Denial inserts nothing. Standalone low-level stores retain caller-owned policy; production composition always installs the validator.
- `src/gateway/handlers/users.rs`: `BrowserViewerGrantRevocation::bind` now receives `Option<Arc<SecurityStore>>` and installs the live-status validator. Only an existing Active User is accepted; absent authority, missing row, read error, or Connection principal denies. The existing deactivation pipeline and retry semantics are unchanged. Existing grant-revocation test binds the in-memory authority; two new regression tests described below.
- `src/gateway/server/browser_live_runtime.rs`: bind the live authority before exposing issuer handlers; runtime test fixtures install an in-memory user store. No new device or connection binding.
- `.superpowers/sdd/2026-10-07-panel-browser-workspace/task-23-acceptance-report.md`: clarify the deliberate User-bound policy limitation and link this evidence; retain PARTIAL status.
- This report and `task3-issue-race-{focused,related,bin-check}.log` plus a diagnostic `task3-issue-race-users-current.diff` under `.analysis/browser-live-runtime/`.

修改保持既有未提交工作；代码边界为以上三个 Rust 文件，无 wire/API 权限形状变化。验收文档只澄清政策及证据。

## Invariant and linearization proof / 不变量与线性化证明

**Invariant:** after a successful user-deactivation operation completes, no grant for that User remains and no further grant can commit while the user remains deactivated. A request already past dispatch identity revalidation must either insert before the same grant-lock sweep (and be revoked by it), or fail issuance without returning a launch URL. Explicit subsequent reactivation permits new grants, not resurrection of revoked grants.

**不变量：**用户停用成功完成后，该 User 无残留 grant；保持停用期间不能提交新 grant。已通过 dispatch 重校验的并发 issue，只能在同一 grant 锁的 sweep 前提交并被清除，或拒绝且无 launch URL。显式重新激活允许新 grant，不恢复旧 grant。

Let D be the committed users-table status write, R the principal sweep under the grant mutex, C the live status read, and I the insertion. Deactivation orders D before R. Issuance holds the grant mutex continuously across C and I. Therefore:

1. If R wins the mutex before issuance, C observes the deactivated status and denies (unless a subsequent explicit reactivation has committed).
2. If C observes Active before D, issuance keeps the mutex through I; R cannot finish before I and sweeps that entry.
3. If D wins before C, C denies even when R is still waiting on other deactivation work.

A final SQL read outside the grant lock would not suffice. Here the critical boundary is the same mutex used by `revoke_for_principal`; there is no await/unlock between the authority read and insertion. `SecurityStore::get_user` reads the actual users row under its connection mutex. `update_user` releases that mutex before any later sweep, so the new lock order (grant -> SQL) has no simultaneous reverse SQL -> grant acquisition in this path. Existing retry deactivation still runs every sweep, not only on active-to-deactivated transitions.

生产启动在 `src/bin/aleph-server/commands/start/mod.rs` 先 `set_security_store(auth_bundle.security_store.clone())`，随后 agent initialization 安装 runtime；`install_browser_live_runtime` 在暴露 issuer 前绑定同一 Arc。状态缺失/读取失败 fail closed，无重复 lifecycle marker，因此重新激活无需清除第二份状态。

Trade-off / 取舍：each issue performs synchronous live SQLite reads while holding the grant mutex. This adds a small serialized read path but avoids duplicated lifecycle/epoch state, activation synchronization, new dependencies, and changes to the deactivation pipeline. The callback must not re-enter the grant store. No benchmark was run.

## Deterministic regression / 确定性回归

`gateway::handlers::users::tests::deactivation_between_dispatch_validation_and_grant_insert_denies`:

1. Use in-memory SecurityStore and pairing store; create Active User/device; bind the production validator and successfully issue an initial grant.
2. Enter real `dispatch_for_connection`. The handler boundary asserts `current_caller_verified_user() == USER`, then signals/parks on paired `TestHooks` Notify barriers before invoking the actual `browser_viewer::handle_issue`.
3. While parked, run real `handle_update(status=deactivated)` **twice** to completion. Assert the initial grant is gone, principal sweep count is zero, and the connection is guest.
4. Release issue, assert generic `denied`, `result == None` (no launch URL), and principal sweep count is zero (no late grant remains). Fresh stale-identity issuance also denies.
5. Reactivate via `handle_update`, issue a new grant successfully, and assert the old grant remains absent.

No timing sleep is used; the 10-second timeout is only a deadlock/watchdog bound. The pause is test-only, but the authority check and lock boundary are production code. The test does not merely count calls: it asserts response, identity demotion, and final grant-store effects.

`gateway::handlers::users::tests::viewer_issue_without_live_user_authority_denies` covers both absent SecurityStore and missing User row, including zero inserted grants.

屏障确实位于 dispatch 验证后、实际 issue handler 插入前；先完成两次真实停用再释放，检查拒绝响应和最终空 store。额外覆盖正常签发、重新激活及缺失 authority/用户的拒绝。

## Safety audit before Cargo / Cargo 前安全审计

Selected paths were statically inspected for home/environment mutation, keyring/vault, SQLite, filesystem effects and deletion. Security/pairing fixtures use in-memory SQLite; selected goal fixtures use owned TempDir SQLite; selected browser control/cookie fixtures use owned TempDir state paths and loopback FakeCdpServer. No selected explicit real-home deletion, keyring/vault operation or HOME/USERPROFILE override was found. Gateway construction can inspect the protocols-directory existence read-only; cfg(test) ProjectStore is in-memory. Build-script writes are worktree/OUT_DIR build artifacts; the existing webchat dist directory was not rebuilt. No Gateway process, Native window or privileged probe was launched.

Before each Cargo invocation, `py -B qa/browser_live/check_memory.py` returned ALLOW at the required **4294967296 bytes**, and a PowerShell process check found **NO_CARGO_RUSTC**. Available bytes: focused **16496611328**, related **16774221824**, bin check **16793616384**. Commands were sequential and offline, with exports:

```sh
CARGO_BUILD_JOBS=1
CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0
CARGO_PROFILE_TEST_CODEGEN_UNITS=256
CARGO_PROFILE_TEST_INCREMENTAL=false
CARGO_INCREMENTAL=0
RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'
```

## Actual verification / 实际验证

1. `cargo test --offline -p alephcore --lib -- gateway::handlers::users::tests::deactivation_between_dispatch_validation_and_grant_insert_denies gateway::handlers::users::tests::viewer_issue_without_live_user_authority_denies --test-threads=1`
   - exit 0; **2 passed**, 0 failed; compile 12m31s, tests 0.03s.
   - `task3-issue-race-focused.log`.
2. `cargo test --offline -p alephcore --lib -- browser::live gateway::handlers::users gateway::server::browser_live_runtime browser::cdp_backend::cookies --test-threads=1`
   - exit 0; **176 passed**, 0 failed: browser::live 127 (including grant/issuer/control), users 38, runtime 5, cookies 6. Tests 4.34s. The new two tests are included here, not 178 unique tests.
   - `task3-issue-race-related.log`.
3. `cargo check --offline -p alephcore --bin aleph-server`
   - exit 0; 3m42s; `task3-issue-race-bin-check.log`.
4. `rustfmt --edition 2021 --config skip_children=true --check` on the three changed Rust files, and `git -c core.autocrlf=true diff --check` on the changed tracked paths: verified separately at final review.

All three Cargo logs contain no warnings. No code changes occurred after these runs; the subsequent edits were documentation only.

## Deliberate limitations / 明确未做

- **User-bound, Frames-only remains policy.** Device revoke, role restamp and connection deactivation do not revoke User-bound grants because no device/connection grant binding is minted. Do not revoke other devices' viewers on a single-device revoke. User deactivation is different and sweeps the entire principal.
- No full-workspace Cargo, clippy, red/mutation run with the guard removed, benchmark, real Gateway/browser E2E, Native/Tauri, cross-platform or admin/AppContainer/WFP validation.
- No Gateway restart, Native launch, capability flip, main-worktree edits, reset, renormalize, cleanup, merge or commit. Unrelated dirty/untracked work, `.analysis`, `nul`, and the real `.aleph` were not removed or rewritten.
- No claim that already-started CDP work is aborted, or that broader Task 3 interleavings and Task 23 blockers are resolved.

未做上述广域/真机验证；不新增设备绑定，不恢复旧授权，不声明整项完成。报告范围内的竞态修复已通过确定性屏障与最终状态断言。
