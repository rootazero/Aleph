# Windows Job Object dynamic ActiveProcessLimit tighten probe (2 -> 1) / Windows Job Object 动态收紧探针

Date / 日期: 2026-10-12 (UTC, taken from the probe's own `start_utc=2026-10-12T02:42:18Z` / `end_utc=...02:42:19Z` in `run1.log`; the `verify-20261007` directory name and the earlier probe report's date are NOT the run date).

## 0. Scope / 范围

**EN.** Narrow second kernel probe. Anonymous Job Object (parent-owned, `KILL_ON_JOB_CLOSE`), std surrogate NO_WINDOW wrapper assigned **before** GO, wrapper starts a normal (non-AppContainer) service that blocks on a controlled stdin barrier (inherited pipe whose write end the parent holds). It tests the one open question left by `windows-job-probe/run2.log` (quota 2 after wrapper exit allows a fork): **tightening the quota 2 -> 1 dynamically while wrapper and service are both alive**. It is NOT AppContainer, NOT `sandbox-init-windows`, NOT the production spawn-suspended barrier; the controlled stdin barrier is a test device, not malicious-child admission. No restricted token / HKCU / profiles / `.aleph` / keyring / WFP / firewall / UAC / browser / Aleph source were touched.

**中文.** 窄范围第二次内核探针：匿名 Job（父进程持有，`KILL_ON_JOB_CLOSE`），std 替身 wrapper（NO_WINDOW，GO 之前已 assign），wrapper 启动普通（非 AppContainer）service，service 阻塞在受控 stdin 屏障上（父进程持有写端的继承管道）。本探针回答 `windows-job-probe/run2.log` 遗留问题：wrapper 与 service **同时存活**时把配额从 2 动态收紧到 1 会怎样。不等价于 AppContainer / 生产 `sandbox-init-windows` / 生产 spawn-suspended 屏障；受控 stdin 屏障不是恶意子进程准入。

## 1. Files / 文件

- `.analysis/browser-live-runtime/windows-job-tighten-probe/job_tighten_probe.rs` (std + raw kernel32 FFI, no Cargo)
- `.analysis/browser-live-runtime/windows-job-tighten-probe/job_tighten_probe.exe` (built with `rustc -O --edition 2021`)
- `.analysis/browser-live-runtime/windows-job-tighten-probe/run1.log` (full output; exit=0)
- Existing `windows-job-probe/*` and its report were not edited.

## 2. Observed results (literal, from run1.log) / 实测结果

| Step | Literal observation |
|---|---|
| Initial limit | Set 2 + KILL_ON_CLOSE OK; read-back `ActiveProcessLimit=2 flags=0x2008` |
| Pre-tighten | exact service handle (`OpenProcess` by pid) `IsProcessInJob`=true; wrapper alive, service alive; members = wrapper + service + conhost; **controlled (non-conhost) = 2**, kernel `Active=3` (conhost counted in Active) |
| Query before tighten | OK `ActiveProcessLimit=2 flags=0x2008` |
| **Set `ActiveProcessLimit=1` (flags `0x2008`, other fields as queried)** | **returned nonzero (success)**, 0 ms; read-back `ActiveProcessLimit=1 flags=0x2008` (KILL_ON_CLOSE preserved) |
| Post-tighten (500 ms) | **wrapper alive, service alive**, `TotalTerminated=0`, Active=3 unchanged. Tightening below the current member count does NOT kill or evict existing members |
| Attempt 1 (service released via `S1`, wrapper+service live) | **REFUSED** `os_error=1816` (ERROR_NOT_ENOUGH_QUOTA); no process entered the job (`Total` stayed 3) |
| Wrapper exits, parent keeps job | members = service + conhost; controlled=1; service still alive |
| Attempt 2 (service released via `S2`, wrapper gone, quota 1) | **REFUSED** `os_error=1816` |
| Cleanup | `TerminateJobObject` + bounded handle waits: `Active=0`; fixture temp dir removed (contained under `%TEMP%\aleph-jobtighten-*`, `exists_now=false`) |

`RESULT: MECHANICS OK (mechanical_fails=0, hypothesis_mismatches=0)`. Hypotheses were not asserted as PASS; they were printed as `[OBSERVED]` with literals, and both happened to match.

## 3. Interpretation / 解读

1. A job's `ActiveProcessLimit` **can be tightened live** by `SetInformationJobObject(JobObjectExtendedLimitInformation)` (Query-modify-Set preserves `KILL_ON_JOB_CLOSE`); it only gates **future** admissions and kills nobody.
2. After tightening to 1, the service is refused its child both while the wrapper is alive and after the wrapper exits (quota 1 consumed by the service itself). This closes the gap seen in the first probe (quota 2 + wrapper exit => fork allowed): a tighten-to-1 after the service is in the job restores a no-fork guarantee for the **normal-process surrogate** in this test.
3. Consistent with the prior probe's conhost accounting: refusals occur when **controlled** members >= limit (here 1 >= 1); prior L2B allowed with controlled 1 < 2 although kernel `Active` included a conhost. This probe cannot separate "conhost exempt" from other hypotheses beyond that consistency, so no stronger claim is made.

## 4. NOT proven / 未证明

- Not AppContainer / restricted token / integrity level; not `sandbox-init-windows` or the production wrapper; not production spawn-suspended admission ordering.
- The tighten was applied after the service was confirmed in the job and held at a cooperative stdin barrier; a malicious child racing the tighten window is NOT modeled (the window between spawn and tighten is unguarded here by design).
- Only one run (n=1); the 500 ms post-tighten settle and timing are not stress-tested. Behaviour on other Windows builds not tested.
- `conhost.exe` accounting hypothesis not isolated (see 3.3).

## 5. Run hygiene / 运行记录

Memory gate `qa/browser_live/check_memory.py` ALLOW before rustc (10.68 GB avail) and before run (11.61 GB avail). All spawned processes NO_WINDOW; no UAC. 150 s overall watchdog (not triggered; run took ~1 s). No process left running; no `aleph-jobtighten-*` temp dir remains. An accidental empty-redirect file `=4294967296` in the worktree root created by my first gate invocation (shell `>=` redirect) was removed.
