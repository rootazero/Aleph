# Windows strict managed companion driver — stage report / Windows 严格托管 companion 驱动阶段报告

Date / 日期: 2026-10-07 · Status / 状态: **PARTIAL — contract only, fail-closed; NOT a pass / 仅契约，保持 fail-closed，**不是**通过**

## 0. Verdict / 结论

> **Update / 更新:** the "fake-API tests not compiled/run" statements below are superseded by `windows-managed-blockers-run.md` (26 passed, incl. the 9 fake-API tests). / 下文「fake-API 测试未编译/未运行」已被 `windows-managed-blockers-run.md` 取代（26 通过，含这 9 个测试）。

**EN.** No Windows managed spawn is enabled. `WindowsSandboxDriver::managed_spawn_preflight` still returns `UnsupportedPolicy`. This stage added (a) a precisely specified, test-pinned launch/admission/teardown contract with no production OS implementation, and (b) a typed, auditable network-capability gate whose current-build answer is "unsupported". The production module was later type-checked by `cargo check -p alephcore`; the 9 fake-API tests were not compiled or run. **Admin network evidence: NONE.** #53 (Windows managed driver) and #48 stay blocked.

**中文.** 未启用任何 Windows 托管 spawn。`managed_spawn_preflight` 仍返回 `UnsupportedPolicy`。本阶段只新增：(a) 一份精确的、由测试固定的「启动/准入/拆除」契约（没有生产 OS 实现）；(b) 一个类型化、可审计的网络能力闸门，当前构建的答案是「不支持」。生产模块后来经 `cargo check -p alephcore` 类型检查；9 个 fake-API 测试未编译、未运行。**管理员网络证据：无。** #53 与 #48 仍被阻塞。

## 1. Files / 文件

| Path | Change |
|---|---|
| `src/sandbox/platforms/windows/managed_launch.rs` | NEW (repository LF working form). `ServiceIdentity`, `NetworkRequest`, `ConfinementCapabilities` (+`CURRENT_CAPABILITIES` = all false), `plan_network_confinement`, `ManagedLaunchApi` trait, `launch_admitted`, `teardown`, 9 fake-API unit tests. `#![allow(dead_code)]` (no production caller by design). |
| `src/sandbox/platforms/windows/mod.rs` | +`mod managed_launch;` |
| `src/sandbox/platforms/windows/driver.rs` | `managed_spawn_preflight` reason text extended (still `feature="managed_spawn"`, still contains "wrapper PID", so the existing refusal test is unaffected). |
| `docs/reports/verify-20261007/windows-managed-driver-stage.md` | this report |

Not touched: `Cargo.toml` (no new dependency/feature), `src/sandbox/managed.rs`, companion `manager/process/protocol/record/tool.rs`, `windows_init/**`, Panel/native/TUI/shared protocol, browser owner semantics. No sandbox-init, AppContainer, firewall/WFP, registry, keyring, UAC or browser was run. The later memory-gated offline `cargo check -p alephcore` type-checked the production module, but did not compile its `cfg(test)` tests. `C:/Users/zou/.aleph` and the running runtime were not touched. No commit/reset. The new contract file is kept in the repository's LF working form.

## 2. Dependency/API inspection (static) / 依赖与 API 静态检查

- `Cargo.toml` Windows deps: `windows-sys = 0.61` with features `Win32_Foundation, Security, Security_Isolation, Security_Authorization, System_Threading, System_JobObjects, Storage_FileSystem, System_Console, System_Pipes, System_Memory, System_SystemServices`. Lock resolves `windows-sys 0.61.2`.
- **WFP is NOT enabled.** Feature `Win32_NetworkManagement_WindowsFilteringPlatform` exists in windows-sys 0.61.2 (`Cargo.toml:124`) and exposes `FwpmEngineOpen0`, `FwpmTransactionBegin0/Commit0`, `FwpmProviderAdd0`, `FwpmSubLayerAdd0`, `FwpmFilterAdd0`, `FwpmFilterGetByKey0`, `FwpmFilter/Provider/SubLayerDeleteByKey0`, `FWPM_LAYER_ALE_AUTH_CONNECT_V4/V6`, `..._LISTEN_V4`, `..._RECV_ACCEPT_V4`, `FWPM_CONDITION_ALE_USER_ID/PACKAGE_ID/IP_REMOTE_PORT`. It also needs `Win32_System_Rpc` (for `SEC_WINNT_AUTH_IDENTITY_W` in `FwpmEngineOpen0`). So the compile surface plausibly exists, but it was **not added or compiled** here.
- Suspended-create/identity surface: `Win32_System_Threading` is enabled (CreateProcessW/AsUserW, ResumeThread, GetProcessTimes); `IsProcessInJob`/`AssignProcessToJobObject` via `Win32_System_JobObjects`. Existing `job.rs::SandboxJob` already sets `ACTIVE_PROCESS|KILL_ON_JOB_CLOSE|DIE_ON_UNHANDLED_EXCEPTION`.
- Existing `sandbox-init-windows` (`windows_init/imp`) is a wrapper that creates the target itself and does not report the real process handle/PID/creation time to the parent. A production launch needs either a new narrowly scoped handoff there or parent-side creation; neither was written (cannot be compiled/verified in this agent).

## 3. Security invariants encoded in `managed_launch.rs` / 已固化的安全不变量

1. **Network decided before any OS object exists** (`launch_admitted` step 1) — test `current_build_refuses_before_any_os_object_is_created` asserts zero API calls.
2. **Order:** `create_job → create_suspended → read_identity → assign_to_job → verify_job_membership → install_confinement → verify_confinement(readback) → resume`. `resume` is last; test `happy_path_...` pins the exact sequence.
3. **Never resume on failure.** Every failure after process creation calls `terminate_and_confirm_empty`; tests assert no `resume` for each failing step, for "not a member", bad readback and zero identity.
4. **Identity = process handle + PID + creation FILETIME**; `ServiceIdentity::new` rejects pid 0 and filetime 0 (a PID-only/wrapper identity is unrepresentable).
5. **Fail-closed teardown ordering:** confinement is removed only after the job is *confirmed* empty; otherwise it is deliberately left installed and the error says so (tests `unconfirmed_empty_job_leaves_confinement_installed`, `teardown_removes_...`).
6. **Network gate** `plan_network_confinement`: `AllowHosts` → always unsupported; empty/zero ports → unsupported; requires `per_run_wfp_confinement && wfp_arbitration_verified && elevated`, each `false` in `CURRENT_CAPABILITIES`. There is no "allow all loopback" outcome and no auto-elevation. Ports are sorted/deduped exact.

## 4. Supported / unsupported now / 当前支持与否

| Case | Behaviour |
|---|---|
| Windows managed spawn (any policy) | **Refused** `UnsupportedPolicy{feature:"managed_spawn"}` (unchanged) |
| `NetworkPolicy::AllowHosts` for managed | Unsupported (typed, `managed_network_allow_hosts`) |
| Loopback-exact confinement | Unsupported in this build (`managed_network_confinement`, then `_arbitration`, `_elevation` once those capabilities exist) |
| Ordinary Windows `execute`/bash sandbox | Unchanged (no code on that path touched) |

## 5. Why not implement WFP now / 为何未实现 WFP

- **Arbitration unverified.** AppContainer loopback isolation is enforced by Windows' own firewall blocks; WFP "block beats permit" across sublayers, so a custom *permit* filter for the container SID cannot be assumed to override it. The only documented route is the per-container loopback exemption (`NetworkIsolationSetAppContainerConfig` / `CheckNetIsolation`), which is an admin/product boundary and was explicitly forbidden to add as an all-app exemption. Whether a per-run-SID exemption + WFP deny/permit pair yields exactly "only 127.0.0.1/::1 + these ports, both CONNECT and LISTEN/RECV_ACCEPT" can only be proven on a real host.
- No code was executed against Windows admin/WFP APIs in this agent. The production module was later type-checked by the memory-gated offline `cargo check -p alephcore`, but no WFP calls were made and the fake-API tests were not compiled or run. Writing unverifiable unsafe FFI that touches persistent system firewall state would violate the stop condition ("only code APIs if compile surface and WFP arbitration can be verified").
- Surrogate probes (`windows-managed-job-probe.md`, `...-tighten-probe.md`) prove only Job-object kernel semantics, not production race, AppContainer, or network.

## 6. Admin experiment still required / 仍需的管理员实验

Run in an **elevated** shell on a disposable host, with a throw-away provider GUID and a fresh AppContainer SID per run (no global/foreign filters; delete everything it created):
1. Create a random AppContainer profile; start a suspended test process (loopback TCP client+listener) under its token; record handle, PID, `GetProcessTimes` creation time; assign to a `KILL_ON_JOB_CLOSE` job and `IsProcessInJob` before `ResumeThread`.
2. In one WFP transaction add provider, sublayer and, for that package SID only, filters at `ALE_AUTH_CONNECT_V4/V6`, `ALE_AUTH_LISTEN_V4/V6`, `ALE_AUTH_RECV_ACCEPT_V4/V6`: permit 127.0.0.1/::1 + exact ports, block everything else (persistent flags). Read back with `FwpmFilterGetByKey0` and compare.
3. Resume. Matrix: connect to allowed port (expect OK); other loopback port, LAN IP, public IP, UDP, IPv6 loopback other port (expect all denied); listen/accept on allowed vs other port; a second AppContainer SID and the unconfined host (expect unaffected).
4. Decide whether a per-SID loopback exemption is required for step 3's "allowed" case (if so, that is an admin-installer product decision, not something the unprivileged gateway may do silently).
5. Teardown: terminate job, confirm empty, delete filters/sublayer/provider/profile; re-enumerate and confirm zero leftovers; kill the test process mid-run to test leftover-cleanup/recovery story for persistent filters.
6. Race stress: many iterations of create-suspended/assign/resume with a forking service under `ActiveProcessLimit`, to settle the wrapper-slot question the probes left open (conhost accounting included).

Only after this succeeds should `CURRENT_CAPABILITIES` flip and a real `ManagedLaunchApi` impl be written, then compiled and tested.

## 7. Why #48 stays blocked / #48 为何仍阻塞

#48 needs a Windows managed companion that is (1) identified by a real handle, PID and creation time, (2) admitted before it can run, and (3) confined to exactly the companion/gateway loopback ports. (1) and (2) are specified but unimplemented; (3) has no safe, verified network mechanism (AllowHosts unsupported, loopback blocked by AppContainer, WFP arbitration unproven). The honest state is fail-closed refusal with typed reasons.

## 8. What was NOT done / 未完成

- The production module type-checked in the later `cargo check -p alephcore`; no clippy or unit-test run was performed, and the 9 fake-API tests were not compiled. No test evidence exists for the ordering assertions.
- No production `ManagedLaunchApi`, no `windows_init` handoff, no WFP/Win32 FFI, no Cargo feature added, no real-host evidence.
- Companion process integration/contract untouched; no check of whether `spawn_managed` callers would need changes once enabled.
- Tests only run on Windows (module is under the `cfg(windows)` platform).
