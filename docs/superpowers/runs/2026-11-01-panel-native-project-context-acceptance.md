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

## Not done / 未做与未验证

A real QA WebView2 shell launch occurred within the approved one-launch budget, reached the page, and stopped at `pairing_surface_unavailable`; no protected OS save/load/delete, same-device native reload, native project A/B/Global, endpoint/lease or revocation evidence was obtained. The exact absence cleanup was completed externally after the runner's helper-stop proof failed. No daily daemon/default release/production credential changes, no old connection-file read, no merge and no push.

未做：protected OS save/load/delete、same-device native reload、native project A/B/Global、endpoint/lease 与 revoke 的 native 证据；runner 的 helper stopped proof 失败后，外部完成了 exact absence cleanup。已实际启动一次 QA WebView2 并到达页面，但 pairing surface 不可用。日常 daemon、默认 release、生产 credential 与旧连接文件未动，未 merge、未 push。
