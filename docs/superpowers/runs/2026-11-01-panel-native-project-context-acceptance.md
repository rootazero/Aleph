# Panel native authenticated project context acceptance run
# Panel native authenticated project context 验收运行记录

- Date / 日期: 2026-11-01
- Branch / 分支: `native-context-qa`; base / 基线: `cb6790e8e` (local main / 本地 main).
- Contract / 契约: `aleph-native-project-context-run-v1`.
- Status / 状态: **BLOCKED — native authenticated project context was not proven / native authenticated project context 未证明**.
- Latest controlled run / 最近受控运行: `node qa/native_ipc/native_project_runner.mjs --repo-root . --execute` launched the QA WebView2 shell within the approved 120-second, one-launch budget. It reached `helper_ready: PASS`, then stopped at `protected_pairing: BLOCKED / pairing_surface_unavailable`; the runner exited `3` as `AMBIGUOUS` because the helper did not emit the required stopped facts. No native trace, protected credential, or Gateway receipt was produced.
- Cleanup / 清理: read-only process inspection found no owned helper or shell. The feature-enabled reaper returned `absent:true` for both exact `delete-exact` and `check-absent`; the owned root was removed afterward. This external cleanup proves resource absence, but it does not upgrade the runner's `AMBIGUOUS` result to PASS.

## Corrected source audit / 源码审计更正

The earlier conclusion that the Panel lacked a project selector was wrong. The literal-string scan missed the protocol constant. The existing path is ProjectsSidebar's project/Global buttons → `DashboardState::select_panel_project` → `aleph_protocol::panel::PANEL_PROJECT_SELECT_METHOD` → the production authenticated Gateway handler. The user delegated the choice of approach; this continuation uses that existing UI rather than adding another business UI or project authority.

此前“Panel 没有项目选择器”的判断错误：字符串扫描漏掉了协议常量。真实链路已存在于 `interfaces/webchat/src/components/sidebar/projects.rs`、`interfaces/webchat/src/context.rs` 和 `shared/protocol/src/panel.rs`。本轮按用户授权选择复用现有 UI；没有新增业务 UI 或项目授权来源。源码文件存在只记为 `source_only`，不冒充 runtime proof。

The controls now expose additive `data-panel-*` observation attributes. The QA monitor has fixed `reveal_projects`, A/B/Global selection and A/B tab actions. They click existing controls only; they do not invoke native IPC, send RPC directly, inject credentials, or accept arbitrary selectors. A regression also proves that an already-open Projects menu is not toggled closed.

真实控件只增加观察属性；QA monitor 的项目动作只点击固定控件。已有菜单展开时的幂等性已通过先 RED 后 GREEN 修复。这些 VM 测试尚未在真实 WebView2 DOM 上运行。

## Remaining blockers and implementation gaps / 真实阻塞与实施缺口

1. **Native launch and endpoint/lease transition / native 启动与 endpoint/lease 切换**: the Panel network ConnectionSection is read-only. The existing native `set_connection_target` command and bundled connection-page paths exist, but the current bounded remote-page DOM driver cannot drive that transition while the old page/lease is active. Command registration alone is not proof of a callable remote UI path. The driver must not call `invoke` or edit markers and then call that an in-flight lease transition. Approved design §3, phase 8 explicitly requires `BLOCKED` for this case.
2. **Native orchestration and cleanup / native 编排与清理**: a real QA WebView2 launch was performed under the approved budget. The shell binary hash matched and the monitor connected, but the page exposed neither the TokenWall nor the Browser pairing notice. The runner therefore obtained no `panel_credential_save`, native trace, helper receipt or protected persistence evidence. The exact cleanup adapter correctly stopped at `helper_stopped` when the helper did not provide its required stopped facts; external CIM plus the feature-enabled exact reaper then proved absence and removed the root.
3. **Endpoint/lease and late-result phases / endpoint/lease 与延迟结果阶段**: the current bounded monitor has no product-supported way to prove an in-flight endpoint/lease transition or selected-project identity at the required boundary. These phases remain `BLOCKED`; no product UI/API expansion is claimed.
4. **No further launch in this run / 本轮不再启动**: the approved 120-second, one-launch budget has been consumed. A new launch would require explicit new authorization.

These results do not establish native acceptance. The fresh shell/WASM/reaper provenance was prepared, but the authenticated pairing path never became available in the real WebView2 page. No product UI/API expansion is claimed or silently approved.

这些结果不能升级为 native 验收通过。fresh shell/WASM/reaper provenance 已准备并使用，真实 WebView2 页面却没有可用的认证 pairing surface；没有暗中新增产品 UI/API，也没有用 helper 控制命令代替真实项目选择。

## Exercised Gateway helper / 已实际运行的 Gateway helper

The QA-only binary links the production `GatewayServer`, token/security stores, `ProjectStore`, Panel RPC dispatch and HTTP publication route. It reuses the shell's owned-root proof, requires a matching `.qa-owned`, and binds a disposable random loopback listener away from `18790`. Mountable fixture modules reuse `community-live.mjs`; they are test community content, not a fake Gateway or a fresh production Panel shell.

QA-only helper 复用生产 Gateway 与授权路径，复用 native owned-root 校验，未授权 root 在写入 fixture 前拒绝。项目名与 driver 固定控件一致；fixture 使用真实可 mount 的 community module。这里的最小静态 HTML 只用于 Node smoke，不证明 Panel/WASM 页面已渲染。

The real Node WebSocket/HTTP smoke now proves:

- server-issued bootstrap pairing authenticates Alice;
- Global discovery, real `projects.list`, explicit A selection and A discovery;
- authenticated A bytes return `200` and contain the mount export;
- asset observation does not establish a device binding; explicit `confirm_pairing` establishes one unambiguous run-created device handle;
- the asset receipt passes the closed protocol with synthetic `device-a`, Alice, project A and the actual Core publication revision;
- B selection preserves the Core revision and refuses A bytes with `404`;
- a held A response was authorized successfully before B, and can arrive as an old `200` after B; this is a delay fixture, not proof of the Panel's native late-result fence;
- explicit Global clearing, rejection of unsupported synthetic selection controls, exact revocation of the actually paired device, and subsequent asset refusal;
- graceful helper shutdown and cleanup of the test-owned process/root.

实际 Node WS/HTTP smoke 证明了上列 Gateway 行为，包括同 revision 切换、真实延迟旧响应和 exact device revoke；这些客户端是 Node，不是 native WebView。旧 WS 实验的根因已定位为 Axum 缺少 `ConnectInfo`，真实 RED 为 `ws_upgrade_status_500`；改为 `into_make_service_with_connect_info::<SocketAddr>()` 后认证成功。此前撤回的挂起实验不计为通过证据。

The helper's feature-gated observer now records typed after-auth `connect`, `select`, and `discovery` outcomes from the real contextual dispatch, plus authenticated refusal after workspace unbind. The helper maps actual Core device/project identifiers to fixed redacted labels only after explicit pairing confirmation; it never records request claims, raw IDs, error text or tokens. HTTP asset receipts remain a separate evidence channel.

QA-only observer 现在挂在真实 contextual dispatch 之后，并记录 typed after-auth `connect`、`select`、`discovery` 结果；workspace unbind 后的 authenticated refusal 也有真实回执。helper 只有在显式 pairing confirmation 后才把实际设备映射到固定脱敏标签；不记录 raw ID、错误文本、请求 claim 或 token。HTTP asset receipt 仍是独立证据通道。

Receipt corrections also replace the unrelated presence/health/config version with Core publication revision, avoid raw device IDs, paginate by sequence cursor, refuse unsupported no-op controls, and bind devices explicitly before observing asset receipts. A server error cannot masquerade as an asset refusal.

回执已修正 Core revision、synthetic handle、游标和显式 binding；观察不再隐式改变 pairing 状态。

## Evidence classification / 证据分层

| Evidence / 证据 | Actual result / 实际结果 | Limit / 边界 |
| --- | --- | --- |
| Pure protocol, evidence, cleanup order / 纯协议与顺序 | PASS in Node tests | No real cleanup or keyring operation / 无真实清理与 keyring |
| DOM driver / DOM 驱动 | PASS in source-extracted JS VM | No WebView2 rendering or IPC / 无真实渲染或 IPC |
| Production Gateway fixture / 生产 Gateway fixture | PASS for the Node WS/HTTP smoke above | Not a native actor / 非 native caller |
| Native credential IPC / native credential IPC | NOT_RUN | WebView2 launched, but pairing surface was unavailable before any credential command / 已启动 WebView2，但 pairing surface 不可用，未执行 credential command |
| OS protected persistence and exact absence / OS 持久化与精确缺失 | NOT_RUN; external exact absence cleanup PASS | No credential was saved; exact reaper absence was proven outside the runner / 未保存 credential；runner 外部已证明 exact absence |
| Real community load in WebView2 / 真实 WebView2 社区加载 | NOT_RUN | The shell reached the page, but authentication did not reach the Panel project UI / 页面已加载，但认证未抵达 Panel 项目 UI |
| Endpoint/lease native phase / native endpoint 与 lease | BLOCKED | No supported current driver transition / 当前 driver 无可用切换链路 |
| Complete desktop runtime / 完整桌面 runtime | BLOCKED/AMBIGUOUS | Pairing was unavailable and the runner could not prove helper stopped facts / pairing 不可用且 runner 未能证明 helper stopped facts |

## Verification / 验证

- `node --test qa/native_ipc/*_test.mjs`: **38 passed, 0 failed, 0 skipped**.
- `node qa/native_ipc/panel_driver.mjs --self-test`: **37/37**, JS VM only / 仅 JS VM。
- `node qa/native_ipc/run-real.mjs --self-test`: **15/15 + 45/45**, pure fakes/owned loopback only / 仅纯 fake 与自有 loopback。
- Guarded `CARGO_BUILD_JOBS=1 cargo build --features native-qa --bin aleph-panel-native-qa-gateway`: **passed**; final incremental build 16.02s. Existing `src/extension/projection.rs` `panel_snapshot` dead-code warning remains.
- Guarded `CARGO_BUILD_JOBS=1 cargo check --bin aleph-server`: **passed**; the default build keeps the observer surface compiled out. Guarded `CARGO_BUILD_JOBS=1 cargo check --features native-qa --bin aleph-panel-native-qa-gateway`: **passed**; only the existing `src/extension/projection.rs` `panel_snapshot` dead-code warning remains.
- Guarded `CARGO_BUILD_JOBS=1 cargo build -p aleph-client --example qa_credential_reaper --features qa-service`: **passed**; the exact reaper used for cleanup was feature-enabled. / 用启用 `qa-service` 的 exact reaper 完成了清理验证。
- Guarded `CARGO_BUILD_JOBS=1 cargo check -p aleph-panel --lib --no-default-features --features panel-modular`: **passed**; final check 1m14s. Existing `PanelTabDescriptor` unused-import warning remains. This is a native-target library check, not a fresh WASM build.
- Every Cargo invocation was preceded by `python qa/browser_live/check_memory.py` from the worktree root; Cargo ran serially in the same dev profile/target. / 每次 Cargo 前均运行 memory gate，固定单 job、同 profile/target。
- Earlier Panel checks timed out during compilation; an intermediate `E0382` from the added `aria-pressed` closure was fixed by removing that unnecessary attribute. Final compilation passed. / 早先超时和中途编译错误均披露，不冒充首次全绿。
- Protocol RED/GREEN regressions now prevent empty, partial or duplicate phases from passing and prevent a blocked phase from hiding cleanup ambiguity. Run identifiers use the full 64-hex digest.
- Provenance regressions exposed that arbitrary `kind/reason` text could enter the ledger. The boundary now accepts only the implemented fixed source-map/hash labels and rejects raw path/token text before storage. / provenance 字段已限制为已实现的固定标签；先 RED 再 GREEN，拒绝将路径或 token 文本写入台账。
- The executable runner was checked in both modes: default mode remains `NOT_RUN/preflight_only`; the authorized native mode launched WebView2 and exited `3` with `AMBIGUOUS`, stopping at `protected_pairing/pairing_surface_unavailable`. It recorded no native trace or credential receipt. / runner 默认模式仍为 `NOT_RUN/preflight_only`；受控 native 模式实际启动 WebView2，因 `protected_pairing/pairing_surface_unavailable` 以 `AMBIGUOUS`、exit 3 结束，未记录 native trace 或 credential receipt。
- Changed Rust files passed targeted `rustfmt --check`; `python qa/browser_live/check_docs.py` and `git diff --check` passed. / 改动 Rust 文件格式、文档检查与 diff 空白检查通过。
- Fixture behavior is verified through the independent built-helper Node WS/HTTP tests rather than a monolithic Core lib-test or a new public test shim. / fixture 使用独立 helper 的真实 WS/HTTP 测试，未增加 public test shim 或重跑 monolithic Core 配置。
- Commits use per-invocation `core.hooksPath=/dev/null` because the existing shared formatting hook is blocked by unrelated baseline files; targeted changed-file formatting passes, and no global hook configuration was changed. / 沿用单次提交绕过既有基线 hook 阻塞，未修改全局 hook 配置；改动文件格式已单独验证。

Ignored local evidence / ignored 本地证据: `.superpowers/sdd/2026-11-01-panel-native-project-context-acceptance/`. It contains compiler logs/status and a redacted preflight report; it does not contain native trace, protected persistence or cleanup proof. / 其中编译日志与脱敏预检不能升级为 native 证据。

## Independent review / 独立审查

The latest completed read-only whole-branch review found six actionable issues: cleanup lifecycle ordering, stale receipt cursor handling, tautological artifact hashes, an unenforced window deadline, a missing cleanup phase, and a connect receipt whose authorization flag was hard-coded. Those issues were corrected after the review: the helper now closes its Gateway listener while retaining a controlled stdin until exact device revoke and EOF; receipt waits require `seq > cursor`; `build.json` must contain and match shell/helper/reaper/WASM digests; the runner checks one shared 120-second deadline; cleanup rewrites the `cleanup` phase from the actual six-step result; and connect observation derives `authorized` from the real response. Focused validation was rerun. This review was substantive and found issues; it is not a clean review.

A further fresh read-only whole-branch review was attempted after the hardening commit, but the reviewer timed out after 600 seconds without returning a report. It is disclosed as a timeout and cannot be called a clean review. The completed validation below and the source audit are the available acceptance evidence for this delivery.


Earlier reviews remain disclosed: `del_mvxlbhy5_sr3x` returned three findings and ended SIGTERM; `del_mvxkn3my_zrsk` ended SIGTERM with no usable findings; and `del_mvxz7fqg_6lk4` ended SIGTERM before findings. None is a clean-exit review.

此前审查仍如实保留：`del_mvxlbhy5_sr3x` 有三项书面 findings 后 SIGTERM；`del_mvxkn3my_zrsk` SIGTERM 且无可用结论；`del_mvxz7fqg_6lk4` 在返回 findings 前 SIGTERM。均不能称 clean review。

三项处理如下：

- **HIGH, fixed after the earlier review / 已在此前 review 后修复**: the earlier asset-only helper receipt gap was closed by the feature-gated Core observer and real helper WS smoke. The observer records only final authenticated connect/select/discovery outcomes and the smoke covers authenticated refusal; these remain helper evidence, not native evidence.
- **MEDIUM, fixed / 已修复**: revealing an already-open Projects menu toggled it closed. Added meaningful failing coverage, then fixed and passed the driver regressions.
- **LOW, fixed / 已修复**: asset observation established pairing bindings. Binding now requires explicit `confirm_pairing`; real Gateway smoke proves an earlier asset yields no receipt/binding and a later confirmed asset yields a valid receipt.

## 2026-11-02 hardening continuation / 2026-11-02 加固续行

This iteration completed the source-backed pairing diagnosis and hardened the QA runner without launching WebView2. The bounded monitor regression uses an authenticated mock page with no `token-wall`, `token-wall-input`, `pairing-notice`, or `pairing-notice-pair-desktop`; it reports `pairing_surface_unavailable`, refuses `ask_pair`, and records no IPC invocation. This is source/VM evidence only. `PairingNotice` remains a workspace Browser viewer `LaunchSection` surface; ordinary fresh Panel has no Browser resource/slot, and `TokenWall` remains governed by the real Gateway `needs_token` verdict and native welcome conditions.

本轮完成了 source-backed pairing diagnosis，并在没有启动 WebView2 的情况下加固 QA runner。bounded monitor 回归测试使用缺少 `token-wall`、`token-wall-input`、`pairing-notice`、`pairing-notice-pair-desktop` 的 authenticated mock page，返回 `pairing_surface_unavailable`，拒绝 `ask_pair`，且不产生 IPC 调用。这只属于 source/VM 证据。`PairingNotice` 仍只属于 workspace Browser viewer 的 `LaunchSection`；普通 fresh Panel 没有 Browser resource/slot；`TokenWall` 仍由真实 Gateway `needs_token` verdict 与 native welcome 条件决定。

The runner now consumes a separately prepared `aleph-native-project-provenance-v1` manifest. It validates closed-shape named SHA-256 values for shell/helper/reaper, Panel WASM, Panel build manifest, and Tauri QA config plus the current 40-hex revision before creating an owned root. Missing, malformed, stale, or mismatched provenance returns structured `NOT_RUN` with empty cleanup/evidence and never prepares a manifest during execution. The current checkout has no fresh `interfaces/webchat/.panel-live/build.json` or independent native provenance manifest, so this iteration remains preflight-only and must not manufacture either artifact.

runner 现在只消费独立准备的 `aleph-native-project-provenance-v1` manifest；在创建 owned root 前校验 shell/helper/reaper、Panel WASM、Panel build manifest、Tauri QA config 的固定命名 SHA-256、闭合结构和当前 40 位 revision。manifest 缺失、损坏、过期或 digest 不匹配时返回结构化 `NOT_RUN`，cleanup/evidence 为空，执行路径不会生成 manifest。当前检出缺少 fresh `interfaces/webchat/.panel-live/build.json` 与独立 native provenance manifest，因此本轮只能停在 preflight，不能制造这些 artifact。

Helper lifecycle now reads the awaited `stopped` facts, keeps the helper alive until exact revoke/EOF, accepts Rust's uncorrelated `not_found` and `already_revoked` errors, and requires final child exit separately. Receipt waits reject `seq <= cursor`, retry uncorrelated `not_found` with the same cursor, and use one absolute business deadline. Cleanup retains the six declared steps and root-last ordering; budget exhaustion is `AMBIGUOUS` even after six successful proofs, and fallback stop runs only when cleanup construction fails before the state machine starts.

helper lifecycle 现在读取 await 后的 `stopped` facts，在 exact revoke/EOF 前保持 helper 可用，并单独验证最终 child exit；适配真实 Rust helper 的无 request-id `not_found` 与 `already_revoked`。receipt wait 拒绝 `seq <= cursor`，对无关联 `not_found` 使用同一 cursor 重试，并共享一个绝对 business deadline。cleanup 仍严格执行六步、root-last；即使六步事实都成功，预算耗尽仍为 `AMBIGUOUS`；只有 cleanup state machine 启动前构造失败时才允许 fallback stop。

Reviewable preflight output from `node qa/native_ipc/native_project_runner.mjs` is `NOT_RUN/preflight_only`, with `launchGate: new_explicit_budget_required`, `webviewLaunches: 0`, `pairingDiagnosis: source_only_runtime_conditions_unobserved`, the exact cleanup sequence, and `artifacts.ok:false/reason:provenance_manifest_missing`. No native trace, credential receipt, or WebView2 launch is claimed in this iteration.

可审阅 preflight 输出为 `NOT_RUN/preflight_only`，包含 `launchGate: new_explicit_budget_required`、`webviewLaunches: 0`、`pairingDiagnosis: source_only_runtime_conditions_unobserved`、完整 cleanup 顺序，以及 `artifacts.ok:false/reason:provenance_manifest_missing`。本轮没有 native trace、credential receipt 或 WebView2 launch 证据。

## Fresh review status / Fresh review 状态

A fresh read-only whole-branch review of commit `0defd4f3f` was dispatched after the implementation commit, but the reviewer timed out after 600 seconds without returning a report. It is disclosed as a timeout and is not a clean review; no findings were silently converted into PASS.

提交 `0defd4f3f` 后已发起 fresh read-only whole-branch review，但 reviewer 在 600 秒后超时且没有返回报告。本次只能记录为 timeout，不能称 clean review；没有把缺失的审查结果默认为 PASS。

## 2026-11-02 authorized fresh-artifact runs / 2026-11-02 获授权 fresh artifact 运行

The user authorized fresh Panel artifacts, an independent provenance manifest, and up to two bounded native launches, each using the runner's 120-second business deadline. No third launch is authorized or performed.

用户授权重新构建 Panel artifact、独立 provenance manifest，并最多进行两次受限 native launch；每次使用 runner 的 120 秒 business deadline。没有授权或执行第三次启动。

Fresh preparation succeeded: `panel-modular` WASM output, native-qa helper, QA shell, and exact credential reaper were built with a memory gate before each Cargo command and `CARGO_BUILD_JOBS=1`. The manifest was written separately with exclusive creation and independently validated at revision `b986ad3b799965b29c15b1b5e9b0efc15fa21408`. No existing manifest was overwritten; one prior manifest from this same authorized preparation was removed only to regenerate after the helper binary changed, then inspected successfully.

fresh preparation 成功：`panel-modular` WASM、native-qa helper、QA shell 与 exact credential reaper 均已构建；每个 Cargo 命令前运行 memory gate，并固定 `CARGO_BUILD_JOBS=1`。manifest 通过独立 exclusive creation 写入，并在 revision `b986ad3b799965b29c15b1b5e9b0efc15fa21408` 独立校验。没有覆盖既有 manifest；helper 二进制变更后，仅删除本次授权准备产生的 manifest 并重新生成，随后检查通过。

The first launch reached the real shell and monitor, then classified the page as `pairing_surface_unavailable` on its immediate DOM inspection. Cleanup proved shell/helper stop but could not prove exact device revocation because pairing had not been confirmed; the helper's previous control path exited before returning a stable no-credential result. The first run is therefore `AMBIGUOUS`, not PASS. This also exposed a boot timing gap: the runner had inspected only once.

第一次启动到达真实 shell 与 monitor，但 immediate DOM inspect 将页面分类为 `pairing_surface_unavailable`。cleanup 证明 shell/helper 已停止，却无法证明 exact device revoke，因为 pairing 尚未确认；当时 helper control path 在返回稳定的 no-credential 结果前退出。因此第一次结果为 `AMBIGUOUS`，不是 PASS。该运行同时暴露了 runner 只 inspect 一次的启动时序缺口。

The focused RED/GREEN repairs were:

- `waitForPairingSurface` polls existing monitor `inspect` for at most 10 seconds and preserves `pairing_surface_unavailable` after the bounded wait; it adds no selector, IPC, hidden UI, or product route.
- The helper's exact revoke returns fixed `already_revoked` when `resolve_device` finds no candidate for a declared binding; multiple candidates still return `ambiguous_device` and terminate fail-closed.
- Node helper/native I/O regressions and the built helper smoke passed; the full Node suite passed 65/65.

修复经过 RED/GREEN：

- `waitForPairingSurface` 使用已有 monitor `inspect` 最多轮询 10 秒，超时后仍返回 `pairing_surface_unavailable`；没有增加 selector、IPC、隐藏 UI 或产品 route。
- helper 的 exact revoke 在声明的 binding 没有候选设备时返回固定 `already_revoked`；多候选仍返回 `ambiguous_device` 并 fail-closed 退出。
- Node helper/native I/O 回归与重建 helper smoke 通过；全套 Node 测试 65/65 通过。

After rebuilding the changed helper and regenerating the independent manifest, the second launch under the then-authorized two-launch budget reached the real shell and completed all six cleanup proofs: shell stopped, helper stopped, exact devices revoked, exact credential deleted, exact absence proved, and owned root removed. The run still ended `BLOCKED`: `protected_pairing` was `BLOCKED/native_run_failed`; there were no helper receipts, native credential trace, or protected persistence proof. The CLI at that point emitted only the fixed redacted `native_run_failed` label, so the underlying runtime cause is unresolved and no finer claim is made. The second run's cleanup is a real `PASS`, but it cannot upgrade the blocked protected phase.

重建变更后的 helper 并重新生成独立 manifest 后，在当时授权的两次启动预算内，第二次启动到达真实 shell，并完成六步 cleanup proof：shell stopped、helper stopped、exact devices revoked、exact credential deleted、exact absence proved、owned root removed。运行仍以 `BLOCKED` 结束：`protected_pairing` 为 `BLOCKED/native_run_failed`；没有 helper receipts、native credential trace 或 protected persistence proof。当时 CLI 只输出固定脱敏的 `native_run_failed`，因此底层 runtime 原因仍未解析，不作更细推断。第二次 cleanup 的确为 `PASS`，但不能升级 blocked protected phase。

The third launch was explicitly authorized later in the same session and rebuilt against current HEAD `aefd44e8c`. The prior generated manifest was removed only because its revision was stale; current Panel, helper, lite shell and reaper artifacts were rebuilt with memory gates and serial Cargo, and a new exclusive provenance manifest was independently validated at the current revision. The third real WebView2 run reached `helper_ready: PASS` and completed all six cleanup proofs, but ended `BLOCKED` with `protected_pairing: BLOCKED/native_run_failed`. It produced no helper messages, receipts, native trace, credential save/load/delete trace, or protected persistence evidence. This is an exact current-HEAD native result, not a PASS. All three authorized launches are consumed; no fourth launch is allowed.

第三次启动是在同一会话中由用户明确授权，并针对当前 HEAD `aefd44e8c` 重新构建。此前 manifest 因 revision 过期而仅删除本次 QA 准备产生的文件；Panel、helper、lite shell 与 reaper 均重新构建，每个 Cargo 命令前执行 memory gate，并重新 exclusive 生成和独立校验 provenance。第三次真实 WebView2 到达 `helper_ready: PASS`，六步 cleanup proof 全部通过，但最终为 `BLOCKED`，`protected_pairing: BLOCKED/native_run_failed`。没有 helper messages、receipts、native trace、credential save/load/delete trace 或 protected persistence 证据。这是针对当前 HEAD 的真实 BLOCKED 结果，不是 PASS。三次授权启动均已消耗，不再允许第四次启动。


精确报告保存在 ignored SDD workspace：`native-launch-1.log`、`native-launch-2.log`、`preflight-before-launch-2.log`、helper lifecycle build output 与 final Cargo logs。这些日志只包含有界/脱敏 runner 输出，不构成额外 native proof。



A real QA WebView2 shell launch occurred within the approved one-launch budget, reached the page, and stopped at `pairing_surface_unavailable`; no protected OS save/load/delete, same-device native reload, native project A/B/Global, endpoint/lease or revocation evidence was obtained. The exact absence cleanup was completed externally after the runner's helper-stop proof failed. No daily daemon/default release/production credential changes, no old connection-file read, no merge and no push.

未做：protected OS save/load/delete、same-device native reload、native project A/B/Global、endpoint/lease 与 revoke 的 native 证据；runner 的 helper stopped proof 失败后，外部完成了 exact absence cleanup。已实际启动一次 QA WebView2 并到达页面，但 pairing surface 不可用。日常 daemon、默认 release、生产 credential 与旧连接文件未动，未 merge、未 push。

## 2026-11-02 evidence-ledger completion / 2026-11-02 证据账本补强完成

This continuation changed only the QA evidence path. `qa/native_ipc/native_project_evidence.mjs` now owns a closed typed `EvidenceEvent` vocabulary, monotonic bounded events, first-failure-wins `primaryFailure`, independent cleanup outcome/reason, operation-scoped `missingLayers`, and a fixed `failureSummary`. `qa/native_ipc/native_project_native_io.mjs` records redacted helper send/ack/stop/revoke, monitor inspect/command, receipt retry/observation, native trace absent/observed, artifact, helper-ready, and cleanup lifecycle boundaries. `qa/native_ipc/native_project_runner.mjs` exposes `events` and `failureSummary` while preserving the four existing public evidence arrays and the exact cleanup contract. No authentication semantics, product UI, Tauri route, selector, browser credential path, actor, endpoint, or cleanup ordering changed.

本次续行只修改 QA evidence path。`qa/native_ipc/native_project_evidence.mjs` 增加闭集 typed `EvidenceEvent`、单调且有上限的事件序列、first-failure-wins `primaryFailure`、独立 cleanup outcome/reason、按 operation 计算的 `missingLayers` 与固定 `failureSummary`。`qa/native_ipc/native_project_native_io.mjs` 记录脱敏的 helper send/ack/stop/revoke、monitor inspect/command、receipt retry/observation、native trace absent/observed、artifact、helper-ready 与 cleanup 生命周期边界。`qa/native_ipc/native_project_runner.mjs` 对外暴露 `events` 与 `failureSummary`，同时保留原有四个 public evidence arrays 和 exact cleanup 契约。没有改变认证语义、产品 UI、Tauri 路径、selector、browser credential path、actor、endpoint 或 cleanup 顺序。

The evidence path has deterministic RED/GREEN coverage. Events reject unknown fields, illegal cursor values, unknown vocabulary, and raw failure text; observer records contain only fixed fields and bounded cursors. An all-empty `helperMessages`/`receipts`/`nativeTrace` report now retains the last reached monitor boundary and reports the operation-scoped missing native trace and receipt layers. Cleanup events cannot replace the business `lastBoundary`; `primaryFailure` remains separate from cleanup, while cleanup incompleteness or budget exhaustion continues to affect the overall run under the existing rules. The Rust QA trace audit found the existing fixed `ALEPH_QA_TRACE` load/save/delete records sufficient, so no Rust or product change was needed.

证据路径已有确定性 RED/GREEN 覆盖。事件会拒绝未知字段、非法 cursor、未知词汇和原始 failure text；observer 记录只有固定字段与有界 cursor。即使 `helperMessages`、`receipts`、`nativeTrace` 全为空，报告仍保留最后到达的 monitor boundary，并按当前 operation 报告缺少的 native trace 与 receipt 层。cleanup event 不能覆盖业务 `lastBoundary`；`primaryFailure` 与 cleanup 独立，cleanup 不完整或预算耗尽仍按既有规则影响整体结果。Rust QA trace 审计确认现有固定 `ALEPH_QA_TRACE` load/save/delete 记录已经足够，因此没有 Rust 或产品代码变更。

Non-native verification completed on current HEAD `cdc4c87f20ac8f1a4a79e7bc4144b9ce721e50da`: root-run `node --test qa/native_ipc/*_test.mjs` passed 72/72; root-run gateway smoke passed 3/3; `python qa/browser_live/check_docs.py` passed; `git diff --check` passed. The lite shell, native QA gateway, exact credential reaper, and `panel-modular` check were rebuilt with a memory gate before each Cargo command and `CARGO_BUILD_JOBS=1`; existing warnings remain in `alephcore` and `PanelTabDescriptor`. The fresh Panel artifact was built from `interfaces/webchat` at build id `3d5da31c1a179363`. An independent exclusive `aleph-native-project-provenance-v1` manifest was prepared and validated against the current revision.

当前 HEAD `cdc4c87f20ac8f1a4a79e7bc4144b9ce721e50da` 的非 native 验证完成：从仓库根运行 `node --test qa/native_ipc/*_test.mjs` 通过 72/72；仓库根 gateway smoke 通过 3/3；`python qa/browser_live/check_docs.py` 通过；`git diff --check` 通过。lite shell、native QA gateway、exact credential reaper 与 `panel-modular` check 均已重建，每个 Cargo 命令前运行 memory gate，并固定 `CARGO_BUILD_JOBS=1`；`alephcore` 与 `PanelTabDescriptor` 的既有 warning 仍存在。fresh Panel artifact 从 `interfaces/webchat` 构建，build id 为 `3d5da31c1a179363`。独立 exclusive `aleph-native-project-provenance-v1` manifest 已生成，并针对当前 revision 独立校验通过。

The resulting source-only preflight remains `NOT_RUN/preflight_only`: `webviewLaunches: 0`, `launchGate: new_explicit_budget_required`, `pairingDiagnosis: source_only_runtime_conditions_unobserved`, `artifacts.ok: true`, and the exact six cleanup steps remain `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, `owned_root_removed`. No WebView2 launch was performed in this continuation. The new failure summary is therefore not a native runtime result; a future launch would still require separate explicit authorization under the one 120-second window rule.

本次续行的 source-only preflight 仍为 `NOT_RUN/preflight_only`：`webviewLaunches: 0`、`launchGate: new_explicit_budget_required`、`pairingDiagnosis: source_only_runtime_conditions_unobserved`、`artifacts.ok: true`，六步 cleanup 仍为 `shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。本次续行没有启动 WebView2，因此新的 failure summary 不是 native runtime 结果；后续 launch 仍须在单个 120 秒窗口规则下另行明确授权。

Implementation and design documents are present at `docs/superpowers/specs/2026-11-02-panel-native-evidence-ledger-design.md` and `docs/superpowers/plans/2026-11-02-panel-native-evidence-ledger-implementation.md`. No commit, merge, push, or native launch was performed in this continuation.

实现与设计文档位于 `docs/superpowers/specs/2026-11-02-panel-native-evidence-ledger-design.md` 和 `docs/superpowers/plans/2026-11-02-panel-native-evidence-ledger-implementation.md`。本次续行没有 commit、merge、push 或 native launch。

## 2026-11-02 authorized WebView2 launch / 2026-11-02 已授权 WebView2 启动

A new explicit 120-second business window was authorized and executed once with the current fresh artifacts. The run reached `helper_ready: PASS`, authenticated the monitor connection, and reached the real WebView2 monitor inspect path. The monitor repeatedly observed `pairing_surface_unavailable`; later the native trace stream delivered an accepted `panel_credential_save` record at the stream layer, and helper acknowledgement was observed. The run then stopped before a receipt was accepted. The public evidence arrays were `helperMessages: []`, `receipts: []`, and `nativeTrace: []`; the event ledger retained the monitor and native-trace lifecycle boundaries. The reported result was `BLOCKED` at `protected_pairing`, with `cleanup: PASS`.

本次获得新的明确 120 秒 business window 并执行一次当前 fresh artifact 的 WebView2 run。运行到达 `helper_ready: PASS`，monitor connection 认证成功，并进入真实 WebView2 monitor inspect 路径。monitor 多次观察到 `pairing_surface_unavailable`；随后 native trace stream 在 stream layer 观察到一个 `panel_credential_save` 记录，helper acknowledgement 也已观察到。运行在 receipt 被接受前停止。public evidence arrays 为 `helperMessages: []`、`receipts: []`、`nativeTrace: []`；event ledger 保留了 monitor 与 native-trace 生命周期边界。最终结果为 `protected_pairing: BLOCKED`，`cleanup: PASS`。

The first report exposed a concrete evidence-ingestion defect rather than a product failure: `NativeTraceStream.take()` intentionally returns the redacted fixed trace fields, while `recordNativeTrace()` still validated the full envelope containing `v`, `auth`, and `pid`. The accepted native stream record was therefore rejected at the JS ledger boundary and surfaced as `internal_unclassified`. The fix now reconstructs the already-authenticated envelope in memory before validation, stores only the redacted trace fields, and maps prefixed validator failures to fixed reasons. RED/GREEN coverage for redacted trace ingestion and fixed failure normalization passes in the full Node suite.

首份报告暴露的是一个具体的 evidence ingestion 缺陷，不是产品失败：`NativeTraceStream.take()` 按设计只返回脱敏后的固定 trace 字段，而 `recordNativeTrace()` 仍要求包含 `v`、`auth`、`pid` 的完整 envelope。因此已被 native stream 接受的记录在 JS ledger 边界被错误拒绝，并显示为 `internal_unclassified`。现在的修复会在内存中为已认证的 record 补回 envelope 后再验证，ledger 只保存脱敏字段，并把带前缀的 validator failure 映射为固定 reason。脱敏 trace ingestion 与固定 failure normalization 的 RED/GREEN 已包含在全量 Node 测试中。

The six cleanup proofs from this launch were all true: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, and `owned_root_removed`. No second WebView2 launch was performed after the fix because the authorization covered one window. Native acceptance remains blocked: the corrected ingestion path has not yet been validated by a subsequent native run, and the run did not obtain accepted native credential, receipt, reload, project, lease, or revocation evidence.

本次 launch 的六个 cleanup proof 全部为 true：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。修复后没有再次启动 WebView2，因为本次授权只覆盖一个窗口。native acceptance 仍为 blocked：修复后的 ingestion path 尚未通过后续 native run 验证，本次运行也没有取得被 ledger 接受的 native credential、receipt、reload、project、lease 或 revocation evidence。

## 2026-11-02 follow-up native run / 2026-11-02 后续 native run

A further direct run was executed to validate the ingestion correction and continue the requested native evidence collection. Run `ee249c43ba1a612e2aa6eeeff3bcba7ac71a5fc7bd0c08df4a35e040fc3d8e85` reached `helper_ready: PASS`, authenticated monitor connection, and observed the real WebView2 inspect path. It reached the native trace boundary and the native stream observed the credential effect, then the runner stopped with `BLOCKED/native_trace_invalid` before accepting a receipt. Cleanup was `PASS`; all six exact cleanup facts were true. No accepted public `nativeTrace`, receipt, reload, Global/A/B project, lease, or revocation evidence was produced.

本次直接执行了后续 run，用于验证 ingestion 修复并继续收集 native evidence。Run `ee249c43ba1a612e2aa6eeeff3bcba7ac71a5fc7bd0c08df4a35e040fc3d8e85` 到达 `helper_ready: PASS`，monitor connection 认证成功，并进入真实 WebView2 inspect 路径。运行到达 native trace boundary，native stream 观察到了 credential effect，随后 runner 以 `BLOCKED/native_trace_invalid` 停止，未接受 receipt。cleanup 为 `PASS`，六个 exact cleanup facts 全部为 true。没有产生被 public `nativeTrace`、receipt、reload、Global/A/B project、lease 或 revocation evidence 接受的记录。

The post-run reproduction used the actual `NativeTraceStream → createTraceHandle → waitNativeTrace → recordNativeTrace` path. It showed that the wire stream cursor is contiguous, while the ledger intentionally records only selected credential effects; requiring adjacent ledger sequence numbers rejects valid traces when another command is consumed by the stream first. The ledger now rejects only duplicate or regressing trace sequence values. A regression test also verifies authenticated redacted ingestion, skipped stream records, `credentialPresent`, and absence of auth/pid from stored evidence. The full root Node suite is now 73/73.

本次 launch 后使用真实 `NativeTraceStream → createTraceHandle → waitNativeTrace → recordNativeTrace` 链路复现。结果确认 wire stream cursor 必须连续，但 ledger 只记录被选中的 credential effect；当 stream 先消费了其他 command 时，要求 ledger 内序号相邻会错误拒绝有效 trace。现在 ledger 只拒绝重复或回退的 trace sequence。新增回归测试覆盖认证后的脱敏 ingestion、跳过 stream record、`credentialPresent`，并确认保存的 evidence 不含 auth/pid。仓库根 Node 全量测试现为 73/73。

No additional WebView2 launch was performed after this correction. Native acceptance remains **BLOCKED**: the corrected ledger path has deterministic non-native coverage, but this follow-up run stopped before an accepted public trace and before receipt, reload, project, lease, and revocation evidence. A further native window would require a new explicit authorization.

## 2026-11-02 reload-boundary validation / 2026-11-02 reload boundary 验证

A newly authorized 120-second business window ran as `d8dc11d113e639162b37c3781a4bf2179ef7fda28f063aca98fa03c5c24c8c7a`. It reached `helper_ready: PASS` and `protected_pairing: PASS`. The public evidence ledger accepted one redacted `panel_credential_save` native trace and one helper receipt. The run then reached `protected_reload` and stopped with `BLOCKED/native_trace_missing` because the returned load record did not satisfy `credentialPresent === true`. No accepted reload, Global/A/B project, lease, or revocation evidence was produced.

本次新授权的 120 秒 business window 运行 ID 为 `d8dc11d113e639162b37c3781a4bf2179ef7fda28f063aca98fa03c5c24c8c7a`。运行到达 `helper_ready: PASS` 和 `protected_pairing: PASS`。public evidence ledger 接受了一个脱敏的 `panel_credential_save` native trace 和一个 helper receipt。随后运行进入 `protected_reload`，因返回的 load record 不满足 `credentialPresent === true`，以 `BLOCKED/native_trace_missing` 停止。没有产生被接受的 reload、Global/A/B project、lease 或 revocation evidence。

All six exact cleanup proofs were `true`: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, and `owned_root_removed`. The run therefore proves accepted save and receipt layers plus complete cleanup, but it does not prove protected persistence or same-device reload.

六个 exact cleanup proof 全部为 `true`：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。因此本次 run 证明了 accepted save 和 receipt layers 以及完整 cleanup，但没有证明 protected persistence 或 same-device reload。

The run also exposed and the QA path now fixes a summary bug. `deriveFailureSummary` scopes the final summary to the first `primaryFailure.phase` and treats the first failure's operation, reason, and `lastBoundary` as authoritative. Reload sets `currentOperation="native_trace"`; when a load record is observed but has `credentialPresent !== true`, the runner records a fixed failed native-trace event with `reason="native_trace_missing"` before throwing. Earlier pairing save/receipt evidence cannot satisfy reload, and later cleanup/helper events cannot move the business boundary. The deterministic regression test is `reload failure keeps its own boundary and does not reuse pairing evidence`.

本次 run 同时暴露并已修复 QA path 的 summary bug。`deriveFailureSummary` 现在以首个 `primaryFailure.phase` 为范围，并以首个 failure 的 operation、reason 和 `lastBoundary` 为权威结果。reload 将 `currentOperation="native_trace"`；当观察到 load record 但 `credentialPresent !== true` 时，runner 会先记录固定 failed native-trace event，`reason="native_trace_missing"`，再抛出异常。更早的 pairing save/receipt evidence 不能满足 reload，后续 cleanup/helper event 也不能移动业务 boundary。确定性回归测试名称为 `reload failure keeps its own boundary and does not reuse pairing evidence`。

After this QA-only correction, root `node --test qa/native_ipc/*_test.mjs` passed **74/74**. The docs gate passed with `check_docs: OK (7 files)`, `git diff --check` passed, and source-only preflight remained `NOT_RUN/preflight_only` with `webviewLaunches: 0`, `launchGate: new_explicit_budget_required`, `artifacts.ok: true`, and the unchanged exact cleanup sequence. No additional WebView2 launch was performed after this correction.

本次 QA-only 修复后，仓库根 `node --test qa/native_ipc/*_test.mjs` 通过 **74/74**。docs gate 为 `check_docs: OK (7 files)`，`git diff --check` 通过；source-only preflight 仍为 `NOT_RUN/preflight_only`，`webviewLaunches: 0`、`launchGate: new_explicit_budget_required`、`artifacts.ok: true`，exact cleanup 顺序不变。修复后没有再次启动 WebView2。

Native acceptance remains **BLOCKED** at the protected reload boundary. The current accepted evidence is limited to credential save, one helper receipt, monitor activity, and exact cleanup. Credential load/persistence, same-device reload, project Global/A/B, lease fencing, and revocation evidence remain unproven. Any further native run requires a new explicit authorization.

本次修复后没有再次启动 WebView2。native acceptance 仍为 **BLOCKED**：修复后的 ledger path 已有确定性的非 native 覆盖，但本次 follow-up run 在 public trace 被接受前停止，也没有取得 receipt、reload、project、lease 和 revocation evidence。若要继续 native 验收，需要新的明确窗口授权。

本次新授权的 120 秒 business window 运行 ID 为 `d8dc11d113e639162b37c3781a4bf2179ef7fda28f063aca98fa03c5c24c8c7a`。运行到达 `helper_ready: PASS` 和 `protected_pairing: PASS`。public evidence ledger 接受了一个脱敏的 `panel_credential_save` native trace 和一个 helper receipt。随后运行进入 `protected_reload`，因返回的 load record 不满足 `credentialPresent === true`，以 `BLOCKED/native_trace_missing` 停止。没有产生被接受的 reload、Global/A/B project、lease 或 revocation evidence。

六个 exact cleanup proof 全部为 `true`：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。因此本次 run 证明了 accepted save 和 receipt layers 以及完整 cleanup，但没有证明 protected persistence 或 same-device reload。

本次 run 同时暴露并已修复 QA path 的 summary bug。`deriveFailureSummary` 现在以首个 `primaryFailure.phase` 为范围，并以首个 failure 的 operation、reason 和 `lastBoundary` 为权威结果。reload 将 `currentOperation="native_trace"`；当观察到 load record 但 `credentialPresent !== true` 时，runner 会先记录固定 failed native-trace event，`reason="native_trace_missing"`，再抛出异常。更早的 pairing save/receipt evidence 不能满足 reload，后续 cleanup/helper event 也不能移动业务 boundary。确定性回归测试名称为 `reload failure keeps its own boundary and does not reuse pairing evidence`。

本次 QA-only 修复后，仓库根 `node --test qa/native_ipc/*_test.mjs` 通过 **74/74**。docs gate 为 `check_docs: OK (7 files)`，`git diff --check` 通过；source-only preflight 仍为 `NOT_RUN/preflight_only`，`webviewLaunches: 0`、`launchGate: new_explicit_budget_required`、`artifacts.ok: true`，exact cleanup 顺序不变。修复后没有再次启动 WebView2。

native acceptance 仍在 protected reload boundary **BLOCKED**。当前已接受的 evidence 仅包括 credential save、一个 helper receipt、monitor activity 和 exact cleanup；credential load/persistence、same-device reload、project Global/A/B、lease fencing 与 revocation evidence 仍未证明。若继续 native run，需要新的明确授权。

## 2026-11-02 monitor-auth boundary run / 2026-11-02 monitor-auth boundary run

A newly authorized 120-second business window ran as `f2b2e117b5287d72befa8e637df4489b0f5017ec1405c3670eb154cc700fb00f`. It reached `helper_ready: PASS` and `protected_pairing: PASS`. The public ledger accepted one redacted `panel_credential_save` native trace and one helper receipt. During `protected_reload`, the reload monitor command was sent and observed, then monitor authentication failed with `protected_reload: BLOCKED/monitor_auth_failed`. The corrected summary preserved `primaryFailure.operation="monitor_command"`, `lastBoundary="monitor_command"`, `failureReason="monitor_auth_failed"`, and did not reuse the earlier pairing evidence as reload evidence. `missingLayers` was empty because this failure occurred at the monitor-auth command boundary before a reload trace operation was entered.

本次新授权的 120 秒 business window 运行 ID 为 `f2b2e117b5287d72befa8e637df4489b0f5017ec1405c3670eb154cc700fb00f`。运行到达 `helper_ready: PASS` 和 `protected_pairing: PASS`。public ledger 接受了一个脱敏的 `panel_credential_save` native trace 和一个 helper receipt。进入 `protected_reload` 后，reload monitor command 已发送并观察到，随后 monitor authentication 失败，结果为 `protected_reload: BLOCKED/monitor_auth_failed`。修复后的 summary 正确保留 `primaryFailure.operation="monitor_command"`、`lastBoundary="monitor_command"` 和 `failureReason="monitor_auth_failed"`，没有把先前 pairing evidence 复用为 reload evidence。由于失败发生在 monitor-auth command boundary，尚未进入 reload trace operation，因此 `missingLayers` 为空。

Cleanup again passed all six exact proofs: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, and `owned_root_removed`. The accepted evidence remains limited to save, one helper receipt, monitor lifecycle activity, and exact cleanup. No accepted load/persistence, same-device reload, project Global/A/B, lease, or revocation evidence was produced. No further WebView2 launch was performed after this run; another native window requires new explicit authorization.

本次 cleanup 再次通过六项 exact proof：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。当前 accepted evidence 仍仅限于 save、一个 helper receipt、monitor 生命周期活动和 exact cleanup。没有产生被接受的 load/persistence、same-device reload、project Global/A/B、lease 或 revocation evidence。本次 run 后没有再次启动 WebView2；若继续 native 验收，需要新的明确授权。

## 2026-11-02 repeated monitor-auth boundary / 2026-11-02 再次 monitor-auth boundary

A further explicitly authorized 120-second run `0c55024a83a795f914084b38c38f2a468a2e5a26db497558c5327dbcb4a491be` again reached `helper_ready: PASS` and `protected_pairing: PASS`. The public ledger accepted one redacted `panel_credential_save` native trace and one helper receipt. During `protected_reload`, the reload monitor command was sent and observed, but monitor authentication failed with `protected_reload: BLOCKED/monitor_auth_failed`. The summary preserved `primaryFailure.operation="monitor_command"`, `lastBoundary="monitor_command"`, `failureReason="monitor_auth_failed"`, and `missingLayers: []`; earlier save and receipt evidence did not satisfy reload.

本次再次明确授权的 120 秒 run `0c55024a83a795f914084b38c38f2a468a2e5a26db497558c5327dbcb4a491be` 同样到达 `helper_ready: PASS` 和 `protected_pairing: PASS`。public ledger 接受了一个脱敏的 `panel_credential_save` native trace 和一个 helper receipt。进入 `protected_reload` 后，reload monitor command 已发送并观察到，但 monitor authentication 失败，结果为 `protected_reload: BLOCKED/monitor_auth_failed`。summary 保留了 `primaryFailure.operation="monitor_command"`、`lastBoundary="monitor_command"`、`failureReason="monitor_auth_failed"` 和 `missingLayers: []`；早先的 save 与 receipt evidence 没有被错误地用于满足 reload。

All six exact cleanup facts were again true: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, and `owned_root_removed`. No accepted load/persistence, same-device reload, project Global/A/B, lease, or revocation evidence was produced. This run consumed the newly authorized window; another native attempt requires new explicit authorization.

六项 exact cleanup facts 再次全部为 true：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。没有产生被接受的 load/persistence、same-device reload、project Global/A/B、lease 或 revocation evidence。本次 run 已消耗新授权窗口；若再次尝试 native 验收，需要新的明确授权。

## 2026-11-02 native evidence boundary / 2026-11-02 native evidence boundary

The explicitly authorized run `d044d278ae098998cb40c8c62166e23124be4f219a3b7d785f761a9fecccbfa9` reached `helper_ready: PASS` and `protected_pairing: PASS`. The ledger accepted one redacted `panel_credential_save` native trace and one helper receipt. It then sent and observed the protected reload monitor command, but stopped at `protected_reload: BLOCKED/monitor_auth_failed`. The final summary preserved `primaryFailure.operation="monitor_command"`, `lastBoundary="monitor_command"`, `failureReason="monitor_auth_failed"`, and `missingLayers: []`; pairing evidence was not reused for reload. All six exact cleanup facts were true: `shell_stopped`, `helper_stopped`, `exact_devices_revoked`, `credential_deleted_exact`, `credential_absent_exact`, and `owned_root_removed`. No accepted load/persistence, same-device reload, project Global/A/B, lease, or revocation evidence was produced.

本次明确授权的 run `d044d278ae098998cb40c8c62166e23124be4f219a3b7d785f761a9fecccbfa9` 到达 `helper_ready: PASS` 和 `protected_pairing: PASS`。ledger 接受了一个脱敏的 `panel_credential_save` native trace 和一个 helper receipt。随后发送并观察到 protected reload monitor command，但在 `protected_reload: BLOCKED/monitor_auth_failed` 停止。最终 summary 保留了 `primaryFailure.operation="monitor_command"`、`lastBoundary="monitor_command"`、`failureReason="monitor_auth_failed"` 和 `missingLayers: []`；pairing evidence 没有被复用为 reload evidence。六项 exact cleanup facts 全部为 true：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。没有产生被接受的 load/persistence、same-device reload、project Global/A/B、lease 或 revocation evidence。

本次 run 已消耗授权窗口；继续 native 验收需要新的明确授权。

## Selector product repair / Selector 产品修复

The separately approved composer execution-tier repair is committed as `bb1bd7cf8`. It adds catalogue feedback/retry, acknowledgement-gated live-session commits, stale-result rejection, and fresh Full confirmation on retry. Panel modular tests passed 1623/1623 and shared send-dial tests passed 5/5. This repair consumed no WebView2 window and does not establish persistence/reload/project/lease/revocation acceptance or resolve `monitor_auth_failed`. Detailed verification, review/hook limits, and artifact status are recorded in [the selector run](2026-11-02-panel-exec-tier-selector-runtime-feedback.md).

独立批准的 composer 执行档位修复已提交为 `bb1bd7cf8`：增加 catalogue 状态与 retry、live session 保存成功后才提交显示值、迟到响应隔离，以及 Full 重试重新确认。Panel modular 测试 1623/1623、共享发送档位测试 5/5 通过。本修复未消耗 WebView2 窗口，不能证明 persistence/reload/project/lease/revocation 验收，也未解决 `monitor_auth_failed`。详细验证、review/hook 限制和 artifact 状态见上述 selector run。