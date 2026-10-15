# Windows managed launch (#53 / #48): production-seam decision / 生产接缝决策

## 0. Verdict / 结论

**EN.** No production seam was landed. `CURRENT_CAPABILITIES` stays all `false`; `WindowsSandboxDriver::spawn_managed` still returns `UnsupportedPolicy`. #53 and #48 are **NOT complete**. Landing a "minimal" parent-created process now would be speculative half-production code, because the real consumer contract cannot hold it (section 2). This document is a blocker report, not an implementation.

**中文.** 未落地任何生产接缝。`CURRENT_CAPABILITIES` 仍全为 `false`；`spawn_managed` 仍返回 `UnsupportedPolicy`。#53 / #48 **未完成**。现在写一个“最小”父进程创建路径只会是推测性的半成品，因为真实消费方契约承接不了它（见第 2 节）。本文是阻塞报告，不是实现。

## 1. What already exists and was verified earlier / 已有且已验证

- `managed_launch.rs`: contract, ordering state machine (`launch_admitted` / abort / teardown), typed `ManagedLaunchBlocker`, pure WFP filter spec builder + readback verifier, `native` primitives (`SuspendedProcess` identity/resume, `terminate_and_confirm_empty`).
- `job.rs`: single-process job, assign, `IsProcessInJob` readback, active count, terminate.
- Evidence: `.analysis/browser-live-runtime/windows-native-suspended-test.log` (20 passed incl. 3 host-token `ping.exe` native tests, ~13.4 GB free), `windows-managed-test.log` (30 passed), `windows-managed-clippy-final.log` (clean). These are logs from earlier runs; **this session ran no cargo command**.
- Those native tests run under the unconfined host token against a system binary, so they prove handle/identity/job mechanics only, not confinement.

## 2. Why a seam cannot be landed safely now / 为何现在不能安全落地

1. **`ManagedChild` is a `tokio::process::Child`** (`src/sandbox/managed.rs:37-56`). The consumer (`src/builtin_tools/browser_tools/companion/process.rs:248-298`) takes `child.stdin/stdout/stderr` and runs `supervise(child, keepalive, ..)` (kill/wait on the tokio child). A process created by parent-side `CreateProcessW` with a restricted token / AppContainer `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` and explicit stdio pipes **cannot be turned into a `tokio::process::Child`** (no from-raw constructor). Conversely, `tokio::process::Command` with `creation_flags(CREATE_SUSPENDED)` would give no main-thread handle for `ResumeThread` through stable API (believed nightly-only, unverified: doc fetch blocked by the SSRF proxy range 198.18.0.0/15) and would run unconfined under the host token. Either path yields a seam the real design cannot use.
2. **Needed cross-module contract change**: abstract `ManagedChild`/`supervise` over a handle-owning child (stdio, wait, kill, identity incl. creation FILETIME), and carry the creation time into `ManagedLaunchRecord` (`record.rs:24` stores PID only). That touches macOS (`seatbelt.rs:981-1023`) and the companion supervisor, outside the allowed edit scope of this task.
3. **Network policy mismatch**: the companion requests `NetworkPolicy::AllowHosts` (+ `spawn_subprocess=false`); `plan_network_confinement` rejects `AllowHosts` unconditionally. A loopback-ports-only mapping needs a product decision and a contract.
4. **No WFP FFI**: `windows-sys` features `Win32_NetworkManagement_WindowsFilteringPlatform` and `Win32_System_Rpc` are not enabled (no verified FFI to justify them). BFE is Running, but nothing was installed or read back.
5. **No admin evidence**: host is not elevated (`TokenElevation=0` per earlier run). WFP-permit vs AppContainer-loopback-block arbitration remains unverified. No AppContainer profile/SID creation or suspended AppContainer parent launch has been attempted.
6. **Wrapper PID**: the sandbox-init wrapper PID is not the service PID; `managed.rs` requires the real service PID. Only a parent-side launcher that owns the real child handle resolves this.

## 3. Edits this session / 本次实际改动

- `src/sandbox/platforms/windows/managed_launch.rs`: replaced the stale STATUS doc comment (it said native primitives were "compile-unverified") with an accurate one pointing at this report. Comment-only; CRLF preserved. Not recompiled (doc comment only).
- This file (new).
- Nothing else. No Panel/Native/HOME/keyring, no Gateway (PID1740 untouched), no elevation, no cargo, no ping/job test run.

## 4. Exact remaining blockers, in order / 剩余阻塞（按序）

1. Design decision (user/architect): refactor `ManagedChild` + `supervise` to a trait/enum over handle-owning child; extend `ManagedLaunchRecord` with creation FILETIME; decide `AllowHosts` -> exact loopback ports mapping.
2. Implement parent-side AppContainer/restricted-token suspended `CreateProcessW` with stdio pipes, behind `ManagedLaunchApi`; feed real PID + creation FILETIME; keep `capabilities` false until step 4.
3. Add WFP FFI (feature flags) and per-run provider/sublayer/filter install + readback + teardown-after-reap, with emergency cleanup.
4. **Elevated, user-approved disposable session** running the opt-in WFP/AppContainer probe (`qa/browser_managed/windows/` still lacks `wfp_probe.exe` and `net_fixture.exe`; only `preflight.ps1` exists). Only real admin WFP + AppContainer + service-PID proof may flip any capability.

## 5. Not done / 未做

No code seam, no WFP/AppContainer experiment, no cargo build/test/clippy/fmt this session, no cross-platform build, no commit; admin proof absent. #53/#48 remain blocked.
