# Phase9 measurement repair / 计量修复记录

> Final runtime checkpoint / 最终运行结果：185504 完整 80 轮 PASS；原 reducer VALID / ACCEPTED；固定审计器 exit0、failures[]、哈希前后相同。默认9文件精确恢复，52源码无漂移。最终证据 SPEC → 不同模型 QUALITY 已完成且无阻塞项；Phase9 PASS、Phase10 READY（已批准的 macOS debug 范围）。下文所有 RUNNING/OPEN/BLOCKED 时间点均为保留历史。当前权威见 [closeout](2026-11-16-phase9-closeout.md) 和 [acceptance](2026-11-18-phase9-acceptance.json)。

> 18:54 follow-up / 后续：184435 已因 SPEC d1915bee 的 final catalog P1 停止并保留为 INVALID（6 rounds；默认资产精确恢复、源完整性 unchanged）。当前 producer 与 verifier 共用 `validate_terminal_catalog`，从首个有效 ownership snapshot 固定宿主名称，逐份检查全部目标 WebKit；未知或仍存活且无归属的记录继续拒绝。45 Panel QA + 15 native-meter tests 通过。新 smoke → fresh 40+40 正在执行；D 仍 OPEN，Phase10 仍 NOT READY。

## Current disposition / 当前状态

**18:43 checkpoint — D remains OPEN; Phase10 is NOT READY.** The unchanged full-block retry183427 is INVALID2 at the same shutdown catalog boundary as181949. Both blocks have complete failure cleanup, exact default restoration and unchanged sources. The strict runtime parser remains unchanged by default. A new terminal-only revalidation can disregard a missing-coalition WebKit catalog record only after its PID was previously observed in this instance and the public identity API now positively reports absence. It reparses the whole catalog so any unknown/live/unproven PID or conflicting coalition still fails. The existing nonce/run closed barrier, host absence, last-real-sample175 ms budget and all-observed-member settlement remain mandatory. / 这不是给未知 WebKit 记录补归属，也不把空样本加入资源曲线。

Actual real-child regression first failed, then passed alive-known rejection, joined-known acceptance, unknown PID rejection and an additional unknown-live record rejection. Forty-five Panel-QA plus fourteen native-meter tests pass; raw evidence is `terminal-remnant-red.log` and `terminal-remnant-green.json`. New paired smoke184214 is running. Incremental SPECd1915bee is reviewing this localized Python change, followed by QUALITY. Native and Panel binaries are unchanged from their separately verified builds. / 新代码仍需真实 smoke、完整新80轮与独立复核，旧轮次不复用。

## Previous retry checkpoint / 前一重跑检查点

**18:36 checkpoint — D remains OPEN; Phase10 is NOT READY.** The public-quit block `isolated-matrix-40-20261118T181949` is INVALID38. At natural close, the strict catalog guard saw GPU PID68223 removed from the host's coalition but still listed as an app record; the prior valid sample contained it. The raw error retains both records. Identity-based cleanup found every observed member absent, no signal required, Gateway/provider joined unforced, exact defaults restored and source integrity unchanged. No ownership tolerance was added. The approved full-block retry is `isolated-matrix-40-20261118T183427`, with identical code and no reused rounds. / 保留原始关闭目录不一致并维持 fail-closed，不将其认作已证明的资源泄漏，也不拼接前38轮。

Ordered SPEC `57a31ab5` and different-model QUALITY `299ad5f1` both report no P0/P1 / OK with notes. QUALITY retains three reporting limits: the NSH `owned_drained` marker denotes successful `run_app` return, while actual kernel exit and observed-member terminal checks independently prove settlement; the compatibility test does not separately name `quit_app`, although the affected binary build compiles the direct public import/call; and historical source integrity is an execution-time assertion. Replaying the174308 auditor against the subsequently changed tree would correctly report main.rs drift, not evidence tampering. Historical original capture/audit receipts remain preserved. / 不为已由主程序构建验证的 API 再添加镜像断言；当前测试范围与历史审计时点明确保留。

## Previous complete-block checkpoint / 前一完整块检查点

**18:21 checkpoint — D remains OPEN; Phase10 is NOT READY.** The complete frozen block `isolated-matrix-40-20261118T174308` contains 80 passing runtime captures, exact 40 Tauri then 40 NSH, exact default restoration and unchanged 52 measurement inputs. The original reducer returned `VALID / NOT_ACCEPTED` (exit 3): only `close_ms:median` failed. Tauri median/p95 were 48.812/73.897 ms; NSH were 55.3695/77.953 ms. The median ratio is 1.134341965, above 1.10; p95 and every other metric pass. The pinned auditor independently reconstructed all 80 captures and reran the unchanged reducer; its only failures are the three acceptance assertions. This valid threshold failure remains preserved in full. / 首个完整有效块仍因真实关闭中位数超标而未通过；不改阈值、不挑选轮次，也不把生命周期 PASS 当作性能 PASS。

Raw close decomposition gives native acceptance-to-closed medians of 0.571 ms for Tauri and 21.764 ms for NSH. A bounded experiment replaces the nonce-bound NSH QA `window.close()` with the pinned public `quit_app()` API. That API closes the standalone window, stops its engine, stops AppKit and returns from `run_app`; all-member settlement, kernel exit status and resource coverage remain required. This measures single-window application exit, matching Tauri's QA application-exit path; it does not cover WindowClose callback semantics. Five affected NSH tests and the build pass; new paired smoke `isolated-matrix-1-20261118T181626` passes both hosts with exact restoration and unchanged sources. One smoke does not establish a performance improvement. A wholly new frozen block `isolated-matrix-40-20261118T181949` is running under owned supervisor 53230. Incremental SPEC `57a31ab5` reports no issues / OK with notes; it confirms full application shutdown and explicitly excludes WindowClose callback coverage. Subsequent different-model QUALITY `299ad5f1` is pending. / 公开退出 API 的局部实验仍需新完整块与独立审查；旧块绝不拼接进新块。

## Previous pacing checkpoint / 前一采样检查点

**17:42 checkpoint — D remains OPEN; Phase10 is NOT READY.** Formal block `isolated-matrix-40-20261118T172707` is INVALID8. Raw timings show an 83.934 ms LaunchServices query followed by a 33.224 ms catch-up sample; the latter violates the unchanged 50–175 ms interval. One controlled delay around actual LaunchServices output reproduced the short interval in `173340`; the one-line scheduler correction passed the same experiment in `173535` and normal paired smoke `173808`, both with exact restoration and unchanged sources. Forty-five Panel-QA and thirteen native-meter tests pass. / 不改资源、时钟、validator 或 reducer；延迟后允许采样计划平移，过长间隔仍判无效。

Current SPEC `15552dd1` reports no issues; ordered different-model QUALITY `766dc97c` is pending. `sampler-pacing-evidence.json` binds the raw reproduction and corrected smoke. The frozen independent auditor was actually run against INVALID8 and rejected missing 80-run coverage, source drift, missing raw counts and missing parity receipt; its automatic before/after hash pin passed. / 无效块负例审计不是性能验收，当前仍需全新完整 40+40 与原 reducer 通过。

## Previous checkpoint / 前一检查点

**17:25 checkpoint — D remains OPEN; Phase10 is NOT READY.** The current SPEC follow-up `4a18e512` reports no issues after re-reading the terminal raw-listing repair. A single `latest_listing_snapshot` is assigned before ordinary and final admission, so final-boundary exceptions retain the actual raw input and its hash. The obsolete post-failure identity diagnostic loop is removed; admission retries, PID census, identity-checked signal authority, sampling limits and reducer thresholds are unchanged. / 终态异常保留实际原始 listing；已观测成员集合与信号授权身份集合继续分离。

Blocks `170629` and `171413` were stopped and retained as INVALID5 and INVALID9 before source changes; their recursive default restoration and source-integrity receipts are true. Fresh paired smoke `isolated-matrix-1-20261118T172003` is PASS2 with exact restoration and unchanged sources. Forty-five Panel-QA and thirteen native-meter tests pass in `terminal-error-green.json`. Ordered different-model QUALITY follow-up `41bcafb5` remains pending. / 不复用无效块成功轮次；静态审查与配对 smoke 不替代完整 80 轮。

The final independent auditor is frozen separately at `full-closure/final-audit-program/verify_current_matrix.py`, SHA-256 `1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94`, with its own manifest. It must be executed against the fresh complete block and its digest checked before final claims. / 固定分析程序与 52 项测量输入承担不同角色；当前尚无有效性能阈值结论。

## Earlier disposition / 较早状态

**17:07 checkpoint — D remains OPEN; Phase10 is NOT READY.** Block `isolated-matrix-40-20261118T164943` is **INVALID24**, ending on a new member's BSD identity query `pid=83645:bytes=0:errno=1`. Its first archived read-only follow-up obtained that same PID's positive startup identity immediately. Defaults and frozen sources restored exactly; no successful round is reused. Admission now retries only that exact member's zero-byte EPERM query up to ten times, 2 ms apart, archives failed queries and the eventual result, and registers only a positive full identity. Persistent EPERM and other errors still fail; neither permission denial nor ESRCH becomes identity or signal authority. This can add up to 18 ms of delay and remains subject to the original cadence/coverage checks. Thirteen native-meter and 44 Panel-QA tests pass (`identity-eperm-green.json`). / 有界重试只等待正向证据，不猜测 EPERM 原因，不放宽采样或 reducer。

Fresh smoke and a new whole block are being run from the same verified `155601` native/Panel builds. Incremental SPEC `5a4e9931` and different-model QUALITY `98a8ff97` are checking the current sources; final ordered confirmation remains required. Claude QUALITY task `1fba0128` failed at the provider's adaptive-thinking configuration and is not a completed review. / 审查失败原样保留，不记为通过；当前没有有效性能阈值结论。

## Historical checkpoints / 历史检查点

**16:50 checkpoint — D remains OPEN; Phase10 is NOT READY.** The controlled same-PID `/bin/sh` → `exec /usr/bin/true` experiment reproduced 31 complete 96-byte TASKINFO results with `errno=0`, resident bytes zero and one live thread. `zero-task-info-exec-probe.json` retains those records. The actual sampler regression failed with `invalid_coalition_task_info:69691:resident_bytes=0:threads=1:virtual_bytes=0`, then passed after removing only the positive-RSS assumption; complete return length, API errors, process ownership, and positive thread-count checks remain enforced. Eleven native-meter and 44 Panel-QA Python tests pass (`zero-kernel-rss-green.json`). / 成功返回中的实际零 RSS 予以保留；不补值，不缩小归属集合，不改 reducer 或阈值。原 INVALID51 未保存字段，不能倒推其具体根因就是 exec。

Fresh paired smoke `isolated-matrix-1-20261118T164825` passed both surfaces with exact recursive default restoration and unchanged frozen sources. A new complete 40+40 block follows from the same verified `155601` native/Panel builds; no prior successful rounds are reused. Incremental SPEC `0d1d6997` is reviewing the sampler correction. Cross-model report `a421c4ab` returned with a zero-field semantic note and inspection limitations; those limitations require follow-up before final review closure. / 新完整块尚无有效阈值结论，审查结论不代替运行验收。

## Earlier checkpoints / 更早检查点

**16:36 checkpoint — D remains OPEN; Phase10 is NOT READY.** Formal block `isolated-matrix-40-20261118T160958` is **INVALID after 51 runs** (40 Tauri and 11 NSH). NSH index10 failed during startup, before any marker, with `invalid_coalition_task_info:59691`; the original diagnostic did not include which resource field was zero. Identity-checked exceptional cleanup settled both registered members, Gateway/model exited 0/-15 without force, and recursive default restoration plus source integrity passed. The complete Tauri baseline and preceding NSH runs are preserved solely as part of this invalid block. / 不拼接成功轮次，不将该错误当作实际泄漏或有效性能超阈值。

Only diagnostic detail has since changed: an invalid full TASKINFO record now reports resident bytes, thread count and virtual bytes. The positive-value check has not been relaxed. Five public suspended-spawn observations and 188,824 TASKINFO calls over 500 short-lived owned processes did not reproduce zero resource fields; fresh controlled native smoke is collecting the missing evidence. Current SPEC `3fb84e2b` found no issues in the origin/metric follow-up; different-model QUALITY/evidence review `a421c4ab` remains active and has been informed of the invalid block. / 原始观测不足时继续定位根因，不猜测并改判据。

**16:10 review disposition —** QUALITY `3750aade` withdrew the earlier libproc P1 after checking the actual public wrapper, and closed the missing-metric P2. `clock-and-metric-green.json` records 44 Python Panel-QA and ten native-meter tests passing. The seven build/test steps in `meter-fixes-20261118T155601/receipt.json` all passed; paired smoke160836 passed before the invalid full block. / 撤回报告和原 BLOCK 原文均保留。

**15:58 historical checkpoint — D remains OPEN; Phase10 is NOT READY.** Block `isolated-matrix-40-20261118T154250` is **INVALID after 11 Tauri runs**. All event clocks were finite and nondecreasing, but the production reconnect witness reported `timeOrigin = 1794988013975` while the other driver markers reported `1794988013974`. Strict origin equality correctly rejected the capture. Default restoration and frozen source integrity both passed. / 完整保留该无效块，不拼接前十轮。

The upstream [WebKit implementation](https://github.com/WebKit/WebKit/blob/main/Source/WebCore/page/Performance.cpp) computes `timeOrigin` by converting the monotonic origin to approximate wall time on each getter call; the retained capture demonstrates a 1 ms difference in this actual run. The driver now captures one document origin and publishes it as the read-only `window.__alephPanelQaTimeOrigin`; the production QA reconnect witness reads that same value and still timestamps the state commit with `performance.now()`. Durations, strict same-origin validation and reducer thresholds remain unchanged. This adds an explicit initialization dependency: the QA driver must install before the production witness can report a reconnect start. / 不估算单向时钟偏移，不修改时长或容差。

`browser-origin-red.log` records the changing-getter regression failing with six origins; `browser-origin-green.log` records all **12 Node tests PASS**. Serial `meter-fixes-20261118T155601` has passed Tauri **190**, NSH **5**, Panel **1568** tests and both native builds; QA/default WASM and fresh runtime validation are pending. Ordered QUALITY `02d12cbf` has been informed of this follow-up. SPEC `c18264aa` closed the missing-owner terminal union, and smoke `154132` passed both surfaces; those results precede this clock repair. / 下列记录均为历史检查点；尚无有效完整 80 轮或性能准入结论。

**15:42 historical follow-up — D remains OPEN.** Independent SPEC `3391b003` closed cleanup-admission ordering, the production reconnect witness, connected queued-wire coverage and the lazy localization boundary, then identified an additional tail shape: the owner record disappears completely while a new member remains in the pinned coalition. Formal block `isolated-matrix-40-20261118T153150` was stopped and marked **INVALID after five completed Tauri runs**; its original checkpoint is preserved separately, with default restoration and source integrity both true. / 不使用该块验收，不拼接已完成轮次。

`parse_terminal_coalition` now shares the catalog parser with normal sampling and unions members from every record carrying the pinned coalition ID. A present owner must match that pin; conflicting coalition membership and malformed member lists are rejected. Both producer admission and raw terminal verification use this path. A real child that survives after its owner record disappears and a hashed raw-tail mutation first demonstrated the gap, then passed: **39 launcher + 10 native-meter tests**, with logs in `full-closure/missing-owner-green.json`. Fresh paired smoke `isolated-matrix-1-20261118T154132` and independent follow-up SPEC `c18264aa` are in progress. / 原生及 Panel 源未因本次纯 Python 计量修复变化，不重复声明新的 Cargo 验证。下列段落保留此前检查点。


**Historical checkpoint: D was not accepted; Phase10 readiness was false.** Formal block `isolated-matrix-40-20261118T141426`, is **INVALID after 15 Tauri runs**: member PID `86609` returned `PROC_PIDTBSDINFO` length zero / `EPERM` while its identity was being registered. That error remains fail-closed; later absence does not prove absence at the failed query. A future failure now retains the current raw listing and bounded read-only identity observations without granting cleanup authority. No valid 80-run reducer input exists yet. / 最新正式块无效，不拼接前 14 轮，不将 EPERM 当作进程消失。

Current repairs address independent SPEC `73278046`: validate pinned coalition and original owner identity before admitting members to cleanup authority; retain raw terminal-tail membership and include it in verifier settlement; start reconnect timing at the production `is_reconnecting` commit; connect actual Gateway queued/accepted/complete wire to the production Panel dispatcher and ChatState. Current focused verification is **43 Python Panel-QA tests, 9 native-meter tests and 11 Node tests PASS**. Full Panel **1568 PASS**, Tauri **190 PASS** and NSH **5 PASS**; native builds passed. QA/default WASM rebuild and current independent SPEC re-review are still in progress. / 连通测试通过不等于性能验收；新 smoke、完整矩阵、后续 QUALITY 与最终证据审计仍待完成。

Historical formal block `isolated-matrix-40-20261118T140410` stopped **INVALID after 12 Tauri runs** when a valid terminal listing retained coalition members after the host left. That exposed the terminal-tail ownership requirement now covered by raw-tail provenance and a real surviving-member negative test. Default restoration and source integrity passed for both invalid blocks. / 完整保留历史原始事件、归属和清理证据。

Historical block `isolated-matrix-40-20261118T130332` stopped **INVALID after 9 Tauri runs**: a process exited between RSS and `ps -M` thread queries. Default assets were restored and the frozen source manifest matched. It is retained in full; its successful runs cannot be spliced into a later block. Further SPEC review required corrections to addressed-send and route timing before a new formal block. / 该正式块已失效，不能继续描述为运行中；最新修复及 smoke 见下文，仍未满足 D 验收。

Base/HEAD: `3892804c112fd6e574166a4c03a7c40a77d0b239`, branch `runtime-proof`. No commit, push, release or platform-policy change is part of this work. / 保持脏工作区及历史证据，不修改保护配置或项目宪法。

## Independent findings and repairs / 独立审查与实际修复

Independent QUALITY `4c6e0124-54fb-4556-bf95-732c8121f033` returned five P1 and one P2. Its original report remains preserved in the Pi subagent output. The following is an implementation/evidence ledger, not a claim that the reviewer has accepted the repairs. / 下表不替代修复后的独立复审。

| Finding | Repair and verification |
|---|---|
| Helper success is not native success / helper 不等于宿主 | Public macOS kqueue `EVFILT_PROC` + `NOTE_EXITSTATUS` records the real native wait status; helper exit is separate. A real same-user, non-child process exiting 42 verifies decoding. / 实取原生退出状态并保留负例。 |
| Host absence does not establish member settlement / 宿主消失不等于全部成员退出 | Retain the union of observed coalition PIDs, then record bounded per-PID terminal snapshots and reject survivors. `resource_scope.complete` and close timing depend on this evidence. / 不接管外部 daemon，不以 PID 名称推断归属。 |
| Startup resource coverage used a later parent origin / 起点错位 | Native and Python use public `CLOCK_MONOTONIC` nanoseconds. A nonce-bound pre-launch PID rendezvous enables a real first coalition sample before releasing a QA-only gate at the existing native application-start boundary. The independent NSWorkspace result must match the native PID. / 原始 native 时钟、样本时钟和共同 origin 均归档验真。 |
| Reconnect ended before state application/render / RPC 不等于渲染 | The production history application publishes a `runtime-qa`-only witness; the observer waits for matching epoch/session and subsequent animation frames before `reconnected`. A delayed-application regression first failed, then passed. / 未复制业务 reducer 或新增业务状态权威。 |
| Resource values were not bound / 数值未绑定原始证据 | Hash the raw ownership records containing the exact per-PID sample; verifier compares the saved sample and recomputes aggregate RSS/thread sums. Aggregate/member mutation regressions reject corruption. / 成员身份和数值分别校验。 |
| Exceptions skipped asset verification and hid primary failure / 异常掩盖 | Asset post-check runs in `finally` while Gateway is alive; its failure is separate from the collector exception. Archive stage/traceback and preserve primary reasons in matrix receipts. Real HTTP-child tests cover collector-only and collector-plus-asset failures. / 资产检查失败仍必须 drain 子进程及删除 fixture。 |

## Controlled startup experiment / 启动格式实证

The first gate experiment deadlocked against the NSWorkspace completion callback and returned PID -1. It was invalid and is preserved. Native PID prepublication broke that dependency, exposing `incomplete_host_coalition` before WebKit startup. / 不再将其归因为平台 WebView 构造问题。

A separate ad-hoc-signed C application launched using public LaunchServices demonstrated the exact output shape on the same machine: coalition 13293 was bare with host PID 28923 alone, became `{ 28923 28925 }` with a known forked child, then became bare after `waitpid`. Raw listings are in `full-closure/singleton-coalition-shape.json`; the controlled application was removed after exit. The parser now accepts the measured singleton representation while retaining duplicate/overlap, owner membership and complete WebKit-role checks. / 这是输出形状实证，不是凭进程名称补造成员。

## Tail observation correction / 终态观测修正

Fresh block `isolated-matrix-1-20261118T125752` reached natural close but remained invalid: last resource 9081.070ms, native closed 9141.008ms, member settlement first checked only at 9336.060ms after helper bookkeeping. Moving the **actual** kernel/member observation immediately after the sampling loop and before helper join removes that avoidable delay. No timestamps were backfilled, no zero samples invented, and the original 50–175ms interval/tail coverage gates remain unchanged. / 观测顺序修复不等于放宽覆盖。

## Fresh evidence / 新证据

- `isolated-matrix-1-20261118T130007/matrix.json`: both surfaces PASS; `restoration.json`: true. Native and helper exit 0, unforced; all observed members absent. / 仅 smoke，不是性能验收。
- Tauri: 90 resource samples, first -7.082ms, last 8885.700ms, observed settlement 8985.678ms. NSH: 96 samples, first -9.602ms, last 9472.386ms, settlement 9576.685ms. Negative first timestamps are actual pre-origin samples taken before gate release. / 两端同一时间轴，首样本先于放行。
- Focused Python: 39 Panel QA + 2 native-meter + 13 runner + 3 lock-delta = **57 passed**; Node observer **4 passed**. Raw summaries are in `full-closure/meter-final-tests.json` and adjacent logs. / 无失败或跳过冒充通过。
- Memory-gated serial native/Panel tests: Tauri **190**, NSH **4 + 1**, Panel **1567**, all passed. QA/default WASM builds and both native builds completed with exit 0; `meter-fixes-20261118T121809/receipt.json`, plus `pid-gate-builds.json` for the later PID-rendezvous host rebuild. / Cargo 逐次内存闸门、offline/locked/jobs1。

The smoke measured NSH RSS 753.328125MB versus Tauri 635.0625MB, and close 105.425ms versus 48.136ms. These single runs do not establish a threshold verdict and are not grounds to alter limits. / 完整矩阵若有效但超阈值，应如实保留 `VALID / NOT_ACCEPTED`。

## SPEC timing follow-up / SPEC 计时复核
Independent SPEC `8e967fc7-faab-48aa-a6fe-189f8ad51cfa` confirmed the common clock, startup gate, controlled singleton interpretation, actual native status, early all-member settlement and rendered reconnect boundaries. It identified two further P1 timing defects and one P2 publication race. / 独立复核不等于正式矩阵验收。

- `input_ms` now ends at the successful `chat.send` response matched by pending RPC ID, with canonical run/session checked against the lifecycle. A new Chat legitimately sends a null session; the server-assigned canonical identity is then authoritative. An explicit requested session must still match. Notification acceptance remains a separate lifecycle event. / Node 和 Python 均保留实际 RED→GREEN 记录，覆盖先后顺序、无关 ID、首次空 session 和外来 run。
- The return-to-Chat start now occurs in a matching capture-phase `popstate` listener, before the production router's normal listener. Dashboard preparation is excluded; visible composer and subsequent frame still establish the endpoint. A regression checks `start < router work < ready`, including work before the next RAF. / 延迟到 RAF 才记起点的旧实现被真实回归拒绝。
- Native PID publication writes a complete same-directory pending file then atomically renames it. Nonce/PID verification remains required. / 不再读取已创建但未写完的 JSON。
- A controlled `/bin/sleep` process exiting during `ps -M` reproduced `coalition_thread_query_failed:`. Only actual `ProcessLookupError` for a requested member reclassifies that failure as `missing_thread_records`; the existing bounded terminal path and original 175ms coverage still apply. / 存活 PID 的查询错误继续拒绝，不造样本、不放宽阈值。

The intermediate addressed-send smoke `isolated-matrix-1-20261118T131918` is **INVALID**, because the initial stricter observer omitted the valid null-session response. Its raw lifecycle, normal cleanup and restoration remain preserved. Current follow-up regressions are **8 Node tests** and **36 launcher tests**, with the immediately preceding combined run **40 Panel QA + 2 native-meter tests** passing. Two hosts were rebuilt in `route-capture-build-20261118T132703/receipt.json`; the associated fresh paired smoke and final SPEC→QUALITY disposition remain pending here. / 该段不是 D PASS；完整 80 轮和最终独立审查仍需完成。

## Browser clock and failure cleanup / 浏览器时钟与异常清理

The next formal block `isolated-matrix-40-20261118T133109` is retained as **INVALID after two runs**. A 248.36 ms resource gap and late terminal observation fail the unchanged coverage contract; these facts do not prove a WebKit leak. No successful prefix is reused.

/ 后续正式块在第二轮因资源采样间隔与尾部覆盖不合格而无效，整块保留，不拼接成功轮，也不把观测延迟写成泄漏。

Independent QUALITY `a219bfec` identified two additional P1 issues: browser intervals used native IPC receipt times, and exceptional collector cleanup did not settle every observed coalition member. Its clarification `bd7a56d6` confirms that startup may end at the native first-frame **receipt**, and close may begin when the native handler accepts the request **before any join/drain**. Route, input and reconnect now use synchronous `performance.now()` endpoints from the same `performance.timeOrigin`; input begins before synchronous input handlers execute. No one-way clock offset is estimated.

/ 浏览器内三个指标使用同一页面时钟，避免 Tauri 消息队列延迟漏计同步工作；启动和关闭仍按已批准的 native 边界计量，不混用时钟。

Resource values now come from public Darwin `proc_pidinfo` for the exact LaunchServices coalition PID set. SDK-compiled layout evidence is retained in `full-closure/libproc-layout.{c,json}`: task info is 96 bytes (RSS offset 8, threads offset 84), BSD info is 136 bytes (start seconds/useconds offsets 120/128). This removes two `ps` subprocesses per sample; it adds an explicit macOS ABI dependency verified against the installed public SDK. Listing and sampling timings are archived to diagnose any future cadence failure; the 50–175 ms limits remain unchanged.

/ 资源归属仍完全来自 OS coalition；libproc 只读取该集合的数值，不按进程名推断归属，也不降低采样要求。

Failure cleanup records PID plus process start time when ownership is observed and rechecks identity before every TERM/KILL. It runs even if the Swift helper has already exited, preserves the original collector error, and records any surviving unregistered member as incomplete. Actual focused checks passed: **41 Panel QA + 6 native meter + 10 Node**, with raw logs in `full-closure/browser-clock-tests.json`. Both native rebuilds passed in `full-closure/browser-clock-build-20261118T135752/receipt.json`.

The real cancellation experiment `full-closure/collector-cancel-20261118T140106/receipt.json` cancelled the addressed matrix Python process after six real samples had observed five owned members. The original `SystemExit: 143` was retained; identity-checked cleanup reached zero survivors with no cleanup errors, Gateway/model joined without forced kill, the fixture was removed, and default assets and source hashes were restored. The preceding experiment's process-selector ambiguity is also retained; its fallback teardown produced independent cleanup evidence, but it is not the successful controlled receipt.

/ 真实取消实验覆盖完整 collector 异常路径，不只验证独立清理函数；它证明本次异常清理成功，不构成性能验收。新的配对 smoke、完整 80 轮、当前有序审查及最终完整性核验仍须独立完成。

## Remaining gates / 未完成项

1. Complete a fresh valid 40 Tauri + 40 NSH block, apply the unchanged reducer (discard ten warmups, retain thirty per surface, p95 rank 29), and retain its actual verdict.
2. Independent current SPEC, then QUALITY and cross-model trail review; resolve concrete defects and rerun invalidated evidence when necessary.
3. Reconcile final source/binary/assets/lock identities, all owned process cleanup, recursive default restoration and final documentation.
4. Do not claim release, cross-platform, full embedded-core bundle, exactly-once effects, or Phase10 readiness from debug evidence alone.

/ 正式验收、最终审查和总账同步尚未完成；不发布、不提交、不推送。
