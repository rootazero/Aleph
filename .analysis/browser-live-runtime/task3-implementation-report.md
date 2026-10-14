# Task 3 implementation report — Agent/Human control across mutation entries

Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256` (not committed; no reset/normalize/cleanup).
Plan: `docs/superpowers/plans/2026-10-07-panel-browser-workspace.md` Task 3.

## Scope reused (no second control / wire shape)

The existing `TargetControl` (per-target serial + `AgentPermit`, lease binding/epoch/permission/controller checks, `run_agent`), the backend `mutate` gate and `ViewerGrantStore::revoke_locked -> TargetControl::revoke(lease, InterventionEnd::Revoked)` are the only machinery used. No protocol type changed (`UserUpdateResult` untouched).

## Changed files (this task only; files also carry earlier uncommitted work)

| File | Change |
|---|---|
| `src/browser/live/grant.rs` | `pub(crate) fn revoke_for_principal(&self, principal: &IssuerIdentity) -> usize` — whole-grant revoke (via `revoke_locked`) of every entry whose `issuer` OR `binding.principal` is the principal. Test `revoke_for_principal_is_whole_grant_revoke_for_that_principal_only`. |
| `src/gateway/handlers/users.rs` | New `BrowserViewerGrantRevocation` (late-bound `Arc<RwLock<Option<ViewerGrantStore>>>`, `bind`, `revoke_user`); new field `UserDeactivationKick::browser_viewer_grants`; `handle_update` deactivation branch calls `kick.browser_viewer_grants.revoke_user(user_id)` after channel-binding revoke (role-only change does NOT revoke). Test `deactivation_revokes_the_users_browser_viewer_grants_only`; `kick_with_pairing` fixture gets inert default. |
| `src/gateway/server/mod.rs` | `GatewayServer::browser_viewer_grants` field (Default) + `pub fn browser_viewer_grant_revocation()`. |
| `src/gateway/server/browser_live_runtime.rs` | `install_browser_live_runtime` binds the slot to `runtime.issuer.grants`. Tests: extended `valid_origin_installs_backend_and_issuer_over_the_producers_stores` (revoke via server handle: other user 0, USER 1, grant gone); new `revocation_handle_taken_before_install_is_bound_by_install`. |
| `src/bin/aleph-server/commands/start/mod.rs` | Production kick gets `server.browser_viewer_grant_revocation()` (kick is built before `install_browser_live_runtime` at `builder/agent_init/mod.rs:555` on the same server, hence the late-bound slot). |
| `src/gateway/handlers/mod.rs` | Inert default registry kick: `browser_viewer_grants: Default::default()`. |
| `src/browser/cdp_backend/mod.rs` | `save_state` now runs through `self.mutate(None, false, cookies::save_state(self, path))`. |
| `src/browser/live/control/tests.rs` | Entry table gains `save-state` / `load-state`; new fixture `live_backend()` / `owner_binding()` over `EngineHandle::live_owner()`; three new tests (below). |

## Decisions

- **save_state is gated.** It calls `ensure_tab` and runs `Runtime.evaluate` (location + localStorage dump) in the page main world — page-observable and racing human input. Same profile-wide gate as the `cookies` read and `load_state`. Existing tempdir save_state tests still pass.
- **record_* unchanged.** `record_start` keeps approval (`check_browser_approval(BrowserRecord)`) + capability gate + `live_owner` ownership; `record_stop`/`record_status` stay ungated as the off-switch (判据 §14), not gated to make a table green.
- **Issuer death = user deactivation** executes canonical whole-grant revoke (control_epoch bump, ws_generation bump, viewer close, controller -> Agent, one `Revoked` summary when a human held control). Human-only permission loss path (`revoke_human_permission`) still retains Frames (existing test `permission_loss_retains_frames_and_failed_summary_closes_control`).

## New/extended tests (assert effects on wire/state)

- `browser::live::control::tests::other_issuer_run_resource_target_neither_blocks_nor_satisfies_the_guard` — human on T1; T2 bound to other user/run/resource: agent `evaluate(T2)` reaches CDP; `evaluate(T1)` -> `HumanHasControl{T1 binding}` with no further `Runtime.evaluate`; foreign lease take/cede -> `Invalid`, revoke -> Err; epoch/controller/lease unchanged.
- `...::take_queued_ahead_of_agent_permit_leaves_no_cdp_write` — serial held; Take queued ahead of agent permit (FIFO); take accepted, agent gets `HumanHasControl`, epoch bumped, zero `Runtime.evaluate` on the wire.
- `...::queued_input_vs_revoke_or_target_close_never_starts_and_summarizes_once` — for revoke, target-close (`invalidate(TargetClosed)`), cede-then-revoke: queued input never dispatches, lease closed, epoch and ws_generation bumped, exactly one `InterventionSummary` with correct `ended_by` and destination, despite a second racing end.
- `backend_mutation_entry_table_refuses_human_before_any_cdp_write` now covers open/navigate/click/type/close-tab/history/evaluate/engine-switch/batch/save-state/load-state (no CDP write, no state file).

## Commands (one at a time, `--offline`, `--test-threads=1`, env `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=256 CARGO_PROFILE_TEST_INCREMENTAL=false CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0 -C codegen-units=256'`; free memory 16.4G/17.2G, no cargo/rustc beforehand)

1. `cargo test --offline -p alephcore --lib browser::live::control -- --test-threads=1` -> exit 0, **15 passed**, 0 warnings (log `task3-impl-control.log`).
2. `cargo test --offline -p alephcore --lib -- browser::live gateway::handlers::users gateway::server::browser_live_runtime browser::cdp_backend::cookies --test-threads=1` -> exit 0, **174 passed** (browser::live 127, users 36, browser_live_runtime 5, cookies 6) (log `task3-impl-related.log`).
3. `cargo check --offline -p alephcore --bin aleph-server` -> exit 0 (log `task3-impl-bin-check.log`).
4. `rustfmt --edition 2021 --check` on changed files: my hunks fixed (3 assert/import layouts, whitespace only, applied after runs 1-2 — not re-compiled). Remaining diffs are pre-existing in files I did not author (`connect.rs`, `auth.rs`, `browser_viewer_route.rs`, `browser_live_runtime.rs:217` ttl_sweeper test). `git diff --check` on changed tracked files: clean.

Safety audit before Cargo: tests use loopback `FakeCdpServer`, `tempdir`, `SecurityStore::in_memory`, cfg(test) in-memory `ProjectStore::shared`; `GatewayServer::new` only probes `$HOME/.aleph/protocols` existence (absent, read-only). No env mutation, keyring, vault, `remove_dir_all`.

## Unresolved / not done

- **Device revoke and role restamp do not revoke viewer grants.** Grants are bound to `IssuerIdentity::User`, not to a device or connection; there is no per-device grant binding to target, and a role change does not change resource ownership. Revoking on device revoke would kill the user's other devices' viewers — a product decision, left as blocker.
- **Connection deactivation hook not wired.** `IssuerIdentity::Connection` grants are never minted (issuer only mints `User`, Frames-only), so there is no grant to revoke there; wiring a caller would be a fake caller.
- **Mid-flight dispatch vs revoke:** `revoke` is synchronous and cannot abort a CDP future already past its first poll; it only guarantees no new start (tested). Not a full barrier test for every interleaving listed in the plan (supersede/expiry/restamp interleavings rely on existing tests `cede_keeps_frames...`, `expiry_closes...`, `stale_agent_waiter...`).
- Gate change for `save_state` was not mutation-tested (red-then-green) in this run.
- Managed / native / full E2E not run; not marked complete. No Native window, Gateway restart, admin/AppContainer/WFP probes, or capability flips. No full workspace test, no clippy.
