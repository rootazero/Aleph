# Windows managed driver: blocker assessment run (Task #53)

## 0. Verdict / 结论

**EN.** Windows managed spawn remains **disabled**: `managed_spawn_preflight` still returns `UnsupportedPolicy`. This process is **not elevated** (`IsInRole(Administrator)=False`, `TokenElevation=0`), so no Windows Filtering Platform (WFP) experiment was attempted and no persistent OS state was written. This run turned the previous opaque refusal into a typed, ordered, host-aware blocker list and ran the previously uncompiled fake-API tests for the first time (all pass).

**中文.** Windows 托管 spawn 仍然**未启用**：`managed_spawn_preflight` 仍返回 `UnsupportedPolicy`。当前进程**未提权**（`IsInRole(Administrator)=False`，`TokenElevation=0`），因此没有尝试任何 WFP 实验，也没有写入任何持久系统状态。本次把原先不透明的拒绝改成有类型、有顺序、感知宿主的阻塞项列表，并首次运行了此前未编译的 fake-API 测试（全部通过）。

## 1. What was attempted / 实际做了什么

| Check | Result |
|---|---|
| Elevation (PowerShell read-only + new Rust probe) | Not elevated; TokenElevation=0 |
| BFE service | Running / Automatic (read-only query) |
| WFP / AppContainer / firewall experiment | **Not attempted** (stop condition: no admin) |
| windows-sys features | `Win32_NetworkManagement_WindowsFilteringPlatform`, `Win32_System_Rpc` not enabled; not added (no verified FFI to justify them) |

## 2. Code changes / 代码改动 (`src/sandbox/platforms/windows/`)

- `elevation.rs` (new): `process_is_elevated() -> Result<bool, String>`; read-only `OpenProcessToken(TOKEN_QUERY)` + `GetTokenInformation(TokenElevation)`, handle closed, errors surfaced.
- `managed_launch.rs`: `HostFacts`, `ManagedLaunchBlocker` (`WrapperPidIsNotServicePid`, `NoProductionLauncher`, `NoPerRunWfpConfinement`, `WfpArbitrationUnverified`, `NotElevated`, `ElevationUnknown`), `assess_managed_launch`, `managed_spawn_preflight_with`. Refusal reason: `N blocker(s): ...; AllowHosts is unsupported`, always naming the wrapper PID.
- `driver.rs`: `managed_spawn_preflight` delegates to `managed_spawn_preflight_with(&HostFacts::observe())`.
- `mod.rs`: `mod elevation;`.
- `CURRENT_CAPABILITIES` **unchanged** (all false). Preflight passes only if every blocker is cleared, which cannot happen in this build.

## 3. Verification (actual) / 实测

Command: `bash .analysis/browser-live-runtime/task51-gate.sh cargo test -p alephcore --lib --offline sandbox::platforms::windows:: -- --test-threads=1` (memory gate >= 4 GiB, `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0`, offline). Filtered tests audited beforehand: pure/fake, plus one read-only token query; no home/keyring/`.aleph` paths.

Log `.analysis/browser-live-runtime/windows-managed-blockers-test.log`: `26 passed; 0 failed; 20717 filtered out`, exit 0, no warnings. Includes the 9 original fake-API tests and 4 new blocker tests. Run twice (second run after context reset, same result).

## 4. Not done / 未做

- No WFP provider/sublayer/filter, no AppContainer, no loopback exemption, no admin evidence; WFP-permit vs AppContainer-loopback-block arbitration remains **unverified**.
- No production suspended-service launcher, no PID+creation-FILETIME capture from a real child, no real Job assign/readback/ResumeThread path.
- No `windows_init` handoff: the sandbox-init wrapper PID is still not the managed service PID.
- No clippy / fmt run; no cross-platform build.
- No Panel/TUI/native/browser changes; no commit.

## 5. Next input needed / 下一步需要

An elevated shell (or a user-approved disposable elevated session) to run the opt-in WFP probe: random provider/sublayer/filter GUIDs, exact disposable AppContainer SID and loopback ports, install before child, read back, teardown after reap, emergency cleanup. Until then #53 and #48 stay blocked.

需要一个提权 shell（或用户批准的一次性提权会话）运行显式 opt-in 的 WFP 探针：随机 GUID、精确的一次性 AppContainer SID 与回环端口、先装后起子进程、回读、回收后拆除、带应急清理。在此之前 #53 与 #48 保持阻塞。
