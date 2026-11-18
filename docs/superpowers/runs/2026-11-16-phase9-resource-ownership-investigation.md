# Phase9 macOS resource ownership investigation / 资源归属调查

Status / 状态: **INSTANCE OWNERSHIP VERIFIED — full 10+30 debug acceptance matrix running (2026-11-18)**.

The earlier OPEN/BLOCKED observations below are retained as experiment history. The sampled-stack correction and current acceptance scope are recorded at the end. / 下文保留历史失败；实际线程栈纠正与当前验收范围见文末。

The runtime collector samples its directly launched native process every 100 ms. This does not cover WebKit XPC processes whose POSIX parent is launchd. It intentionally records `resource_scope.complete = false`; the metrics exporter refuses to turn these partial samples into an accepted parity document.

当前采集器每 100 ms 采样直接启动的原生进程。WebKit XPC 进程的 POSIX 父进程为 launchd，不能用普通父子 PID 关系证明归属，因此该采样并不完整；指标导出继续拒绝这类数据。

## Observations / 已观测事实

- The controlled NSH window created real GPU, Networking and Web Content processes. LaunchServices lists their application labels and PIDs, but the controlled shell and these processes did not expose a `parentASN` or usable per-application coalition in the captured records. Matching labels alone does not establish ownership when another instance could exist.
- `responsibility_get_pid_responsible_for_pid` attributed both shell and engine processes to the surrounding Terminal process. It therefore cannot separate this run from other applications launched there.
- A private, temporary spawn-attribute experiment did not produce an accepted executable launch. Its results are diagnostic failures, not an adopted launcher mechanism.
- Public `/usr/bin/open -W -n` probes used an isolated generated application bundle. Separate probes tested a symlink executable, a copied executable, an ad hoc signed temporary bundle, and a signed bundle under an owned account directory. Each returned LaunchServices error `-10810`. The targeted unified log identifies `RBSRequestErrorDomain Code=5` with underlying `NSPOSIXErrorDomain Code=1 "Operation not permitted"` and `Launchd job spawn failed`. No system policy, entitlement or security setting was changed. The temporary bundles were removed and their probe processes exited.

以上实验没有得到可用于 100 ms 全量采样的系统归属契约。没有把名称匹配当成 PID ownership，也没有把部分 RSS/线程数当成完整数据。

## Evidence and boundary / 证据与边界

Raw probe outputs remain in the task workspace `.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/`: `ownership-info-*`, `launch-app*-result.json`, `launch-app*-supervisor.log` and the temporary probe scripts. These are diagnostic artifacts; full application census output is not copied into tracked acceptance records.

The debug parity gate remains open. Ten warmups and thirty measured runs cannot establish the required RSS/thread thresholds while ownership is incomplete. No accepted performance comparison, release-profile conclusion, or cross-platform result is claimed. A valid measured threshold miss must still be recorded as `NOT_ACCEPTED`; incomplete ownership is an evidence-validity limitation, not such a miss.

资源归属尚未闭合，因此没有声称通过 debug parity，也没有执行或声称 release 性能验收。后续应在可验证的系统应用实例归属下补齐完整采样，再执行 Tauri first / NSH second 的 10 + 30 次完整比较。

## Follow-up probe / 后续探针 (2026-11-17)

A controlled live NSH process was queried through the documented `lsappinfo info -only <key> ASN:<...>` interface while its WebKit processes were present. The shell PID was `13569`; LaunchServices listed GPU `13571`, Networking `13573`, and Web Content `13572`. For all four records, `parentasn`, `parentasnwasinferred`, and `coalition` returned `[ NULL ]`. `-only pid` returned the expected process identifier, so the empty ownership fields are not a failed ASN lookup. The captured raw query is [parentasn-probe.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/parentasn-probe.json). After the shell's owned process group exited, a fresh LaunchServices listing contained none of those NSH records. This confirms cleanup for that run, not ownership during sampling.

The account's `launchctl procinfo <pid>` probe exited 1 with `This subcommand requires root privileges: procinfo`. The installed public SDK headers under `MacOSX.sdk/usr/include` contain no declarations for `PROC_PIDCOALITIONINFO`, `coalition_info_resource_usage`, or `responsibility_get_pid_responsible_for_pid`. No elevated command, private API, entitlement, or system-policy change was attempted.

A direct execution from a temporary `.app` under `/tmp` was denied with `sandbox_extension_issue_file_to_process failed ... (Operation not permitted)`. A second direct execution from an account-owned temporary `.app` was registered by LaunchServices, but did not start WebKit within the 40-second observation because it used an unreachable local Gateway URL. It therefore does not establish WebKit ownership. The result is [bundled-home-parentasn-probe.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/bundled-home-parentasn-probe.json). These probes do not change the gate: `resource_scope.complete` remains false, and no 10-warmup/30-measured run or parity reducer acceptance is claimed.

一轮受控 NSH 实例运行期间，通过文档化的 `lsappinfo info -only <key> ASN:<...>` 查询宿主及 WebKit 进程。宿主 PID 为 `13569`；LaunchServices 列出 GPU `13571`、Networking `13573`、Web Content `13572`。四个记录的 `parentasn`、`parentasnwasinferred`、`coalition` 均为 `[ NULL ]`。`-only pid` 返回了预期 PID，因此这不是 ASN 查询失败。原始结果见 [parentasn-probe.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/parentasn-probe.json)。宿主进程组退出后重新查询，已没有这些 NSH 记录。这只确认本次关闭后的清理，不证明采样期间的实例归属。

当前账户执行 `launchctl procinfo <pid>` 以退出码 1 失败，错误为 `This subcommand requires root privileges: procinfo`。已安装的 `MacOSX.sdk/usr/include` 公共头文件没有 `PROC_PIDCOALITIONINFO`、`coalition_info_resource_usage` 或 `responsibility_get_pid_responsible_for_pid` 的声明。没有尝试提权命令、私有 API、entitlement 或系统策略变更。

从 `/tmp` 直接执行临时 `.app` 被系统拒绝，错误为 `sandbox_extension_issue_file_to_process failed ... (Operation not permitted)`。从账户目录直接执行时 LaunchServices 能识别该 `.app`，但测试使用不可达的本地 Gateway URL，在 40 秒观察期内没有启动 WebKit。因此这不能证明 WebKit 归属。结果见 [bundled-home-parentasn-probe.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/bundled-home-parentasn-probe.json)。这些探针不改变验收闸门：`resource_scope.complete` 仍为 false，也没有执行 10 次预热加 30 次测量或声称 reducer 验收。

## NSWorkspace LaunchServices probe / NSWorkspace LaunchServices 探针 (2026-11-17)

The public AppKit SDK exposes `NSWorkspace.openApplication(at:configuration:)` with `createsNewApplicationInstance` and environment injection, so the hypothesis was that a LaunchServices-owned NSH instance might expose an ASN inherited by its WebKit XPC processes. The same public headers say `NSRunningApplication` tracks user applications, not every process; public `WKWebView`/`WKNavigationDelegate` headers expose process-termination notification but no WebContent PID. Therefore an app launch alone would not satisfy D; the experiment required a non-inferred parent-ASN link for each relevant process.

The exact debug NSH executable was placed in a temporary, ad-hoc-signed `.app` with a valid plist. `codesign --verify --deep --strict` and `plutil -lint` passed, but `NSWorkspace.openApplication` returned: `The application “launchservices-nsh-probe” could not be launched because it is corrupt.` `spctl --assess --type execute --verbose=4 --raw` returned exit 3 and `rejected` (`assessment:remote=true`, `assessment:verdict=false`). Unified logging records `rejecting due to lack of matching active rule`; `security find-identity -v -p codesigning` found `0 valid identities found`. The NSH process therefore never launched and no WebKit child records were available to query. The isolated Gateway exited 0, the model child joined without force, and its fixture was removed.

Raw evidence: [policy-checks.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/policy-checks.json), [NSWorkspace launcher stderr](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/launcher.stderr.log), and [Gateway cleanup receipt](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/gateway/gateway-cleanup.json). No Gatekeeper rule, trust setting, entitlement, or system policy was changed. This result neither proves nor disproves ASN inheritance; it establishes that this machine cannot run the probe without an approved trusted signature or launch environment. D remains OPEN/BLOCKED, and no 10+30 run was made.

公开 AppKit SDK 提供 `NSWorkspace.openApplication(at:configuration:)`，支持创建新实例并注入环境，因此假设通过 LaunchServices 启动的 NSH 可能向 WebKit XPC 子进程提供可继承的 ASN。相同公开头文件注明 `NSRunningApplication` 只跟踪用户应用，不覆盖所有进程；公开 `WKWebView`/`WKNavigationDelegate` 只提供 WebContent 进程终止通知，没有 WebContent PID。故“应用启动成功”本身仍不足以闭合 D，必须逐个取得非推断的 parent-ASN 关联。

本次把现有 debug NSH 二进制放入临时 ad-hoc 签名 `.app`，签名结构检查和 plist 校验通过，但 `NSWorkspace.openApplication` 返回 `The application “launchservices-nsh-probe” could not be launched because it is corrupt.`。`spctl --assess --type execute --verbose=4 --raw` 退出码 3 并拒绝执行（`assessment:remote=true`、`assessment:verdict=false`）。系统日志记录 `rejecting due to lack of matching active rule`；`security find-identity -v -p codesigning` 显示 `0 valid identities found`。因此 NSH 未启动，也没有 WebKit 子进程可供查询。隔离 Gateway 退出码 0，模型子进程未强制结束，夹具已删除。

原始证据见 [policy-checks.json](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/policy-checks.json)、[NSWorkspace launcher stderr](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/launcher.stderr.log) 和 [Gateway cleanup receipt](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/nsworkspace-owned-launch/gateway/gateway-cleanup.json)。未修改 Gatekeeper 规则、信任设置、entitlement 或系统策略。该结果不能证明或否定 ASN 继承；它证明当前机器没有获批可信签名或启动环境时无法执行这个探针。D 仍为 OPEN/BLOCKED，没有运行 10+30 矩阵。

## Coalition ownership follow-up / Coalition 归属后续 (2026-11-17)

A public `NSWorkspace` two-instance NSH experiment did establish a useful instance boundary through `lsappinfo list`: host PIDs `29372` and `29384` received coalition IDs `10711` and `10713`, with disjoint member sets; each set contained its own GPU, Networking and Web Content records across 24 stable snapshots, and all observed records disappeared after both helpers exited. A 100-sample NSH run also measured 100 ms ownership samples with gaps from 90.109 ms to 109.964 ms. This is retained as a public, non-private ownership feasibility result, not yet as D acceptance because the production runtime collector still needs marker binding and both surfaces.

The direct Tauri binary emitted the complete QA lifecycle, but its PID was inside the surrounding Terminal/Pi coalition and therefore cannot satisfy per-instance D. A real `ai.aleph.desktop` bundle launched by public `NSWorkspace` did receive an independent coalition and produced hundreds of ownership observations in an earlier probe. However, the official Tauri bundle and the manually assembled equivalent both stopped before `builder.build()` returned when launched through LaunchServices: the retained QA root contains `.phase9-boot` and `.phase9-setup-before`, but no setup-after, navigation, WebKit data, or Panel marker. Activating the app and fixing the bundle working directory did not change this. The exact diagnostic receipt is [real Tauri app probe](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/real-tauri-app-probe/receipt.json); the official bundle was built at `.phase9-target/debug/bundle/macos/Aleph.app`. These runs are failures of the launch path, not accepted performance samples.

QA assets were restored after the experiments. The default WASM digest is `2a208e2aa4f5de4f13d6f250442d7a1b0535a2398a15a119cd8f957f26ac2719`, and the launcher suite remains 28/28. The coalition parser continues to fail closed on incomplete host membership; no parser relaxation or system-wide process matching was introduced.

公开 `NSWorkspace` 双实例 NSH 实验通过 `lsappinfo list` 得到了有用的实例边界：宿主 PID `29372`、`29384` 分别获得 coalition `10711`、`10713`，成员集合互不相交；每个集合在 24 个稳定快照中都包含自己的 GPU、Networking 和 Web Content，两个 helper 退出后观测记录全部消失。另一次 NSH 100 次采样的间隔为 90.109–109.964 ms。这是公开 API 下的归属可行性证据，尚不是 D 验收，因为生产 collector 仍需绑定完整 marker，并同时覆盖两个 surface。

直接启动 Tauri binary 能产生完整 QA 生命周期，但其 PID 属于外层 Terminal/Pi coalition，不能满足逐实例 D。通过公开 `NSWorkspace` 启动真实 `ai.aleph.desktop` bundle 时，较早探针确实得到独立 coalition 并产生数百次归属采样；但正式 Tauri bundle 和手工等价 bundle 经 LaunchServices 启动时都在 `builder.build()` 返回前停住：保留的 QA root 有 `.phase9-boot` 和 `.phase9-setup-before`，没有 setup-after、WebKit data 或 Panel marker。激活应用和固定 bundle cwd 均未改变结果。精确诊断回执见 [real Tauri app probe](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/real-tauri-app-probe/receipt.json)；正式 bundle 位于 `.phase9-target/debug/bundle/macos/Aleph.app`。这些运行是启动路径失败，不是可接受的性能样本。

实验结束后已恢复 QA 之前的默认 Panel 产物。默认 WASM 摘要为 `2a208e2aa4f5de4f13d6f250442d7a1b0535a2398a15a119cd8f957f26ac2719`，launcher 套件仍为 28/28。coalition parser 继续对不完整宿主成员集合 fail-closed，没有放宽判据，也没有引入全局进程名匹配。

## Ready-boundary follow-up / Ready 边界后续 (2026-11-17)

A QA-only experiment deferred `build_main_window` from setup to `RunEvent::Ready`. The retained root produced `.phase9-ready`, proving that the Ready event arrived, and also created `webview2`; `builder.build()` still did not return within the 15-second probe. The helper timed out and terminated with exit 1, with zero complete ownership samples and zero Panel markers. This distinguishes the failure from a missing setup/run-loop event: the block remains inside Tauri WebView construction under the public LaunchServices launch path. The experiment was removed afterward, and the default Panel assets were restored.

一次 QA-only 实验把 `build_main_window` 从 setup 推迟到 `RunEvent::Ready`。保留的 root 生成了 `.phase9-ready`，证明 Ready 事件已经到达，同时创建了 `webview2`；但 15 秒探针内 `builder.build()` 仍未返回。helper 超时后退出码为 1，完整 ownership sample 和 Panel marker 都为 0。这排除了 setup/run-loop 事件尚未到达的假设：当前公开 LaunchServices 启动路径的阻塞仍发生在 Tauri WebView 构造阶段。实验随后已撤回，默认 Panel 产物已恢复。

A follow-up source audit found that the removed Ready experiment had left a QA-only duplicate `RunEvent::Ready` window-build branch behind while setup already built the window. That stale branch was deleted; `rustfmt --edition 2024 --check --config skip_children=true`, `git diff --check`, the 28-test launcher suite, and the memory-gated QA shell build all pass afterward. This cleanup does not alter production behavior, ownership criteria, thresholds, or the blocked D disposition.

后续源码审计发现，已撤回的 Ready 实验仍遗留了一个 QA-only 重复 `RunEvent::Ready` 窗口构造分支，而 setup 已经完成窗口构造。该遗留分支已删除；之后 `rustfmt --edition 2024 --check --config skip_children=true`、`git diff --check`、28 项 launcher 测试和内存门禁后的 QA shell build 均通过。本次清理没有改变生产行为、归属判据、阈值或 D 的阻塞结论。

## Sampled-stack correction / 实际线程栈纠正 (2026-11-18)

The earlier conclusion that WebView construction blocked was incorrect. A controlled public `/usr/bin/sample` capture located the main thread at `build_main_window → phase9_qa::install → boot_diagnostic → OpenOptions::open`, before `builder.build()`. The QA capture file was on the external workspace volume. Moving marker and ownership capture to the local owned temporary directory allowed the genuine Tauri bundle to emit its full Panel sequence through the same public NSWorkspace launch. Capture files are archived only after client exit; no OS policy, entitlement, private API or ownership predicate changed. The boot diagnostic's literal backslash-n was also corrected to a real newline.

之前“WebView 构造阻塞”的结论错误。受控 `/usr/bin/sample` 实际定位为 `build_main_window → phase9_qa::install → boot_diagnostic → OpenOptions::open`，尚未到达 `builder.build()`；QA capture 位于外置工作卷。将 marker 与 ownership capture 移到受控的本地临时目录后，真实 Tauri bundle 经同一公开 NSWorkspace 路径产生完整 Panel 事件。客户端退出后才归档文件；未修改系统策略、entitlement、私有 API 或归属判据。另将 boot 诊断中的字面反斜杠 n 修复为真正换行。

The frozen paired smoke [isolated-matrix-1-20261118T113903](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/isolated-matrix-1-20261118T113903/matrix.json) passes both Tauri and NSH with real signed executable provenance, raw LaunchServices coalition snapshots, every member's RSS/thread sample, strict 50–175ms cadence/lifecycle coverage, 79 nonce-bound events per surface, four actual Panel ingress rejections, natural post-cleanup close, fresh Gateway/model drain and exact default asset restoration. It establishes the ownership/runtime path; it is not the 10+30 performance verdict. Native monotonic clocks measure latency; per-run resource metrics are peak RSS and maximum threads. Queued identity is checked if observed, and the receipt explicitly records when it was absent.

冻结后的双端 smoke [isolated-matrix-1-20261118T113903](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/isolated-matrix-1-20261118T113903/matrix.json) 两端通过：真实签名 executable 来源、原始 LaunchServices coalition、每个成员的 RSS/线程样本、50–175ms 采样与生命周期覆盖、每端 79 条 nonce 关联事件、四种真实 Panel ingress 拒绝、清理后的自然关闭、全新 Gateway/model 回收及默认资产精确恢复。此证据闭合归属和运行路径，不等于 10+30 性能通过。延迟使用原生单调时钟，资源指标使用每轮峰值 RSS 与最大线程数。观察到 queued 时验证关联，未出现则明确记录。

The unchanged full Tauri-first/NSH-second 40+40 matrix is now running at [isolated-matrix-40-20261118T114101](../../../.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/isolated-matrix-40-20261118T114101/matrix.json), with a fresh isolated server/session per run and the existing reducer. Phase10 readiness remains false until valid measurements meet the original thresholds and final review closes. / 完整 40+40 矩阵正在运行，每轮全新服务/会话，保持原 reducer；有效数据满足原阈值且完成最终审查前，Phase10 readiness 仍为 false。
