# Windows Job Object ActiveProcessLimit probe / Windows Job Object 进程配额探针

Date / 日期: 2026-10-07 (probe run on this Windows host)

## 0. Scope / 范围 (read first / 先读)

**EN.** This probe only characterises the **kernel semantics of an anonymous Job Object with `ActiveProcessLimit` + `KILL_ON_JOB_CLOSE`**, using a *std-only surrogate* wrapper and a *controlled* service (all one tiny test exe, `job_probe.exe`). It is **NOT** AppContainer-equivalent (no AppContainer, no restricted token, no integrity level, no DACL/profile). It is **NOT** a proof about the production `sandbox-init-windows`, the production companion/wrapper, or the real race between "wrapper starts" and "parent assigns the job". Nothing in Aleph source was edited; `sandbox-init-windows` was not executed.

**中文.** 本探针只刻画「匿名 Job Object + `ActiveProcessLimit` + `KILL_ON_JOB_CLOSE`」的**内核语义**，用一个仅依赖 std 的*替身* wrapper 和一个*受控* service（同一个小测试 exe）。它**不等价于 AppContainer**（无 AppContainer、无受限 token、无完整性级别、无 DACL/profile），**也不是**对生产 `sandbox-init-windows`、生产 companion/wrapper、或"wrapper 启动 vs 父进程分配 job"真实竞态的证明。未改动任何 Aleph 源码，未运行 `sandbox-init-windows`。

## 1. Files / 文件

All new, under the worktree `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/`:

| Path | Role |
|---|---|
| `.analysis/browser-live-runtime/windows-job-probe/job_probe.rs` | source (std + raw kernel32 `extern "system"`) |
| `.analysis/browser-live-runtime/windows-job-probe/job_probe.exe` | built binary (sha256 prefix `3144a27c60ae9488`) |
| `.analysis/browser-live-runtime/windows-job-probe/run1.log` | first run (pre-conhost-aware assertions; 2 FAILs, see §4) |
| `.analysis/browser-live-runtime/windows-job-probe/run2.log` | final run, **ALL PASS** (60 lines) |
| `docs/reports/verify-20261007/windows-managed-job-probe.md` | this report |

## 2. Commands / 命令

```
py -3 qa/browser_live/check_memory.py            # before every compile/run: available ~12.5GB >= 4GB -> ALLOW
cd .analysis/browser-live-runtime/windows-job-probe
rustc --edition 2021 -O -C codegen-units=1 job_probe.rs -o job_probe.exe
timeout 150 ./job_probe.exe > run2.log           # exit=0
```
No Cargo, no aleph-server, no Chrome/WebView, no `.aleph`/HOME/USERPROFILE/HKCU/keyring, no profiles/firewall/WFP/admin.

## 3. Design / 设计

- Parent (probe, **not** in job) creates an anonymous job (`CreateJobObjectW(NULL,NULL)`), sets `LimitFlags = ACTIVE_PROCESS(0x8)|KILL_ON_JOB_CLOSE(0x2000)` via `SetInformationJobObject(class 9)`, reads it back.
- Parent spawns surrogate **wrapper** (`CREATE_NO_WINDOW`, stdin piped). Wrapper writes `wrapper-ready` then **blocks reading stdin `GO\n`**. Parent verifies wrapper is not in the job and has spawned nothing, calls `AssignProcessToJobObject`, verifies membership, **then** writes `GO`. (This is a *controlled* ordering for the probe only; the production race is NOT claimed solved.)
- L1 `ActiveProcessLimit=1`: wrapper (1 slot used) attempts to spawn a trivial `child`.
- L2A `limit=2`, wrapper stays alive: wrapper spawns `service` (slot 2); service attempts one further child; parent opens the service by PID and queries it; parent kills it via the owned handle.
- L2B `limit=2`, wrapper exits early: same, but wrapper exits after the service's first attempt while the parent keeps the job and service alive; service then attempts a second child.
- All waits bounded; `Drop` guard does `TerminateJobObject` + `CloseHandle(job)`; fixture root is `%TEMP%\aleph-jobprobe-<pid>-<nanos>` (removed only after checking parent==temp_dir and name prefix).

## 4. Results / 结果 (run2.log, literal)

**L1 — PASS.** Wrapper's attempted target spawn:
`REFUSED os_error=1816 (配额不足，无法处理此命令。 (os error 1816))` = `ERROR_NOT_ENOUGH_QUOTA`. Job accounting afterwards `TotalProcesses=1` (the refused process never entered the job; spawn returned an error rather than creating-then-killing; `TotalTerminated=0`).

**L2A — PASS.** Wrapper spawned service OK; service's further child: `REFUSED os_error=1816`. Job members: `[wrapper job_probe.exe, service job_probe.exe, conhost.exe]` (Active=3 Total=3 — see conhost note). Service killed via parent handle: `wait=0x0 exit=0x4a4a`; wrapper observed and finished; final Active=0.

**L2B — PASS.** Attempt before wrapper exit: `REFUSED os_error=1816`. Wrapper exited (`WaitForSingleObject==0`) while parent kept job. Members after: `[service job_probe.exe, conhost.exe]`. Attempt **after** wrapper exit: `SPAWNED pid=14024 exit_code=0x0` (the freed slot was used). Final accounting `Total=5 Active=2`.

**Owner handle accessibility (NORMAL, non-AppContainer service only).** Parent `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|SYNCHRONIZE|PROCESS_TERMINATE)` on the service PID succeeded in both L2A and L2B; `IsProcessInJob==true`; `GetProcessTimes` creation FILETIME non-zero (e.g. 134362455300273190); `GetExitCodeProcess` = `259 (STILL_ACTIVE)` while alive; `WaitForSingleObject(0)` = `0x102 (WAIT_TIMEOUT)`; after `TerminateProcess(0x4A4A)` wait=`0x0` and exit code `0x4a4a`. **No claim is made about handle access to an AppContainer / low-IL process** — not tested.

### Unexpected finding: `conhost.exe` / 意外发现

Run 1 asserted `Active==2` and failed (`Active=3 Total=3`, later `Active=2` after wrapper exit with `Total=3`). Enumerating `JobObjectBasicProcessIdList` + `QueryFullProcessImageNameW` showed the extra member is a **`conhost.exe`** that the kernel/console subsystem places in the job for each console child created *inside* the job (even with `CREATE_NO_WINDOW`). It counts in `ActiveProcesses`/`TotalProcesses` accounting but **did not count toward the limit** (Active=3 with limit 2; and L2B's second spawn succeeded with Active=2=limit because only 1 non-conhost member remained). Consequences: (a) never use `ActiveProcesses` equality as an assertion/invariant of "number of user processes"; enumerate members and filter, or tolerate `conhost.exe`; (b) the wrapper itself had no conhost in the job because it was started *before* assignment. Run 2 assertions count non-conhost members and pass. (Whether a non-console/GUI-subsystem service avoids the conhost was not tested.)

## 5. Conclusions / 结论

1. **PASS (kernel-observed):** with `ActiveProcessLimit=N`, once N user processes are live in the job, `CreateProcess` from a member fails with **os error 1816** and no process is created or counted. This is real kernel enforcement, not a script echo.
2. **Race caveat / 竞态注意:** `limit=2` is **not** an absolute "no fork" guarantee. If the wrapper exits (or any member exits) while the job lives, the freed slot lets the service fork (L2B attempt 2 succeeded). A wrapper-as-slot-holder design must keep the wrapper alive for the service's lifetime (or the parent must treat wrapper exit as service termination: `TerminateJobObject`). `KILL_ON_JOB_CLOSE` gives cleanup-on-parent-death only.
3. The probe's stdin-GO ordering shows that if the parent assigns the job before releasing the wrapper, no child escapes the job. It says nothing about production wrappers that start themselves before assignment.
4. Parent keeps full control through its own job handle (`TerminateJobObject`, `IsProcessInJob`, member list) and, for a normal-token service, via `OpenProcess` by PID (creation time recorded to mitigate PID reuse; here the handle is opened right after start and verified in-job).
5. `conhost.exe` appears as a hidden extra job member — account for it.

## 6. Cleanup & global side effects / 清理与全局副作用

- Owned cleanup: job handles terminated/closed, `Child` handles waited, service handle closed; fixture root `%TEMP%\aleph-jobprobe-*` removed (`removed=true exists_now=false`); `tasklist` after each run shows no `job_probe` processes; no leftover `aleph-jobprobe-*` in TEMP.
- Global OS writes: **none** beyond ordinary process creation and files under the temp fixture root and the new `.analysis/.../windows-job-probe/` dir. APIs used: `CreateJobObjectW`, `SetInformationJobObject`, `QueryInformationJobObject`, `AssignProcessToJobObject`, `IsProcessInJob`, `TerminateJobObject`, `OpenProcess`, `GetProcessTimes`, `GetExitCodeProcess`, `WaitForSingleObject`, `TerminateProcess`, `CloseHandle`, `GetCurrentProcess`, `QueryFullProcessImageNameW`. No registry, keyring, firewall/WFP, AppContainer/profile/DACL APIs.

## 7. Uncertainty / 不确定性

- Single host, single OS build; result was reproduced once more after adding member enumeration (run1 vs run2) but not stress-tested.
- Surrogate only: no AppContainer, no restricted token, no integrity-level/handle-access tests, no production wrapper/companion, no real wrapper-start-vs-assign race.
- Behaviour when the probe itself already runs inside an outer job (nested jobs, Win8+) was implicitly tolerated (assign succeeded) but not examined separately.
- conhost exemption from the limit is inferred from counts (Active 3 > limit 2), not from documentation.
