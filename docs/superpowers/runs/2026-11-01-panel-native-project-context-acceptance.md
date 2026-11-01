# Panel native authenticated project context acceptance run
# Panel native authenticated project context 验收运行记录

- Date / 日期: 2026-11-01
- Branch / 分支: `native-context-qa`; base / 基线: `cb6790e8e` (local main / 本地 main).
- Contract / 契约: `aleph-native-project-context-run-v1`.
- Status / 状态: **BLOCKED — native acceptance remains incomplete / native 验收未完成**.
- Current preflight / 当前预检: `node qa/native_ipc/native_project_run.mjs --repo-root . --execute` exits `5`, reason `endpoint_transition_requires_approved_native_driver`, before creating a listener, shell, credential, or native-owned root.
- The CLI is a source-only preflight, not an implemented native execution runner. / 该 CLI 仅为源码预检，不是已完成的 native 执行 runner。

## Corrected source audit / 源码审计更正

The earlier conclusion that the Panel lacked a project selector was wrong. The literal-string scan missed the protocol constant. The existing path is ProjectsSidebar's project/Global buttons → `DashboardState::select_panel_project` → `aleph_protocol::panel::PANEL_PROJECT_SELECT_METHOD` → the production authenticated Gateway handler. The user delegated the choice of approach; this continuation uses that existing UI rather than adding another business UI or project authority.

此前“Panel 没有项目选择器”的判断错误：字符串扫描漏掉了协议常量。真实链路已存在于 `interfaces/webchat/src/components/sidebar/projects.rs`、`interfaces/webchat/src/context.rs` 和 `shared/protocol/src/panel.rs`。本轮按用户授权选择复用现有 UI；没有新增业务 UI 或项目授权来源。源码文件存在只记为 `source_only`，不冒充 runtime proof。

The controls now expose additive `data-panel-*` observation attributes. The QA monitor has fixed `reveal_projects`, A/B/Global selection and A/B tab actions. They click existing controls only; they do not invoke native IPC, send RPC directly, inject credentials, or accept arbitrary selectors. A regression also proves that an already-open Projects menu is not toggled closed.

真实控件只增加观察属性；QA monitor 的项目动作只点击固定控件。已有菜单展开时的幂等性已通过先 RED 后 GREEN 修复。这些 VM 测试尚未在真实 WebView2 DOM 上运行。

## Remaining blockers and implementation gaps / 真实阻塞与实施缺口

1. **Native endpoint/lease transition / native endpoint 与 lease 切换**: the Panel network ConnectionSection is read-only. The existing native `set_connection_target` command and bundled connection-page paths exist, but the current bounded remote-page DOM driver cannot drive that transition while the old page/lease is active. Command registration alone is not proof of a callable remote UI path. The driver must not call `invoke` or edit markers and then call that an in-flight lease transition. Approved design §3, phase 8 explicitly requires `BLOCKED` for this case.
2. **Trusted RPC receipts / 可信 RPC 回执**: the helper currently records actual authenticated HTTP asset outcomes only. It does not yet emit after-auth `connect/select/discovery/revoke` RPC receipts. Existing contextual dispatch owns those checks; no public test shim or `actor=None` path was added to bypass them. This is unfinished implementation, not proof that the product cannot support it.
3. **Native orchestration / native 编排未完成**: evidence aggregation and cleanup-order modules have pure tests, but the new runner has not wired the real process manager, monitor, protected credential reaper, independent absence verifier, or native phase sequence. Earlier task-completed markers overstated this scope; the remaining work is explicitly tracked.
4. **Launch prerequisites / 启动前置条件**: no numeric GUI window budget was established in the approved design. No current shell/WASM/reaper served-provenance bundle or native launch preflight was completed. Historical budgets and launch permissions were not reused.

These are not grounds to fabricate native evidence. No Windows Tauri/WebView2 window or OS credential operation was launched in this continuation. No further product UI/API expansion is claimed or silently approved.

以上缺口均保留为未完成项。本轮没有启动 Tauri/WebView2 或访问 OS credential；没有伪造 native marker、覆盖 `__TAURI_INTERNALS__`，也没有用 helper 控制命令代替真实项目选择。

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

Receipt corrections also replace the unrelated presence/health/config version with Core publication revision, avoid raw device IDs, paginate by sequence cursor, refuse unsupported no-op controls, and bind devices explicitly before observing asset receipts. A server error cannot masquerade as an asset refusal.

回执已修正 Core revision、synthetic handle、游标和显式 binding；观察不再隐式改变 pairing 状态。helper 仍缺少上述 RPC receipts，不能称完整 native proof。

## Evidence classification / 证据分层

| Evidence / 证据 | Actual result / 实际结果 | Limit / 边界 |
| --- | --- | --- |
| Pure protocol, evidence, cleanup order / 纯协议与顺序 | PASS in Node tests | No real cleanup or keyring operation / 无真实清理与 keyring |
| DOM driver / DOM 驱动 | PASS in source-extracted JS VM | No WebView2 rendering or IPC / 无真实渲染或 IPC |
| Production Gateway fixture / 生产 Gateway fixture | PASS for the Node WS/HTTP smoke above | Not a native actor / 非 native caller |
| Native credential IPC / native credential IPC | NOT_RUN | No shell child trace / 无 shell trace |
| OS protected persistence and exact absence / OS 持久化与精确缺失 | NOT_RUN | No OS-store operation / 未访问 OS store |
| Real community load in WebView2 / 真实 WebView2 社区加载 | NOT_RUN | No fresh served shell/WASM proof / 无新 shell/WASM 证据 |
| Endpoint/lease native phase / native endpoint 与 lease | BLOCKED | No supported current driver transition / 当前 driver 无可用切换链路 |
| Complete desktop runtime / 完整桌面 runtime | NOT_RUN | Required native phases incomplete / native 阶段未完成 |

## Verification / 验证

- `node --test qa/native_ipc/native_project_protocol_test.mjs qa/native_ipc/native_project_evidence_test.mjs qa/native_ipc/native_project_run_test.mjs qa/native_ipc/native_project_driver_test.mjs qa/native_ipc/native_project_gateway_smoke_test.mjs`: **32 passed, 0 failed, 0 skipped**.
- `node qa/native_ipc/panel_driver.mjs --self-test`: **37/37**, JS VM only / 仅 JS VM。
- `node qa/native_ipc/run-real.mjs --self-test`: **15/15 + 45/45**, pure fakes/owned loopback only / 仅纯 fake 与自有 loopback。
- Guarded `CARGO_BUILD_JOBS=1 cargo build --features native-qa --bin aleph-panel-native-qa-gateway`: **passed**; final incremental build 12.74s. Existing `src/extension/projection.rs` `panel_snapshot` dead-code warning remains.
- Guarded `CARGO_BUILD_JOBS=1 cargo check -p aleph-panel --lib --no-default-features --features panel-modular`: **passed**; final check 1m14s. Existing `PanelTabDescriptor` unused-import warning remains. This is a native-target library check, not a fresh WASM build.
- Every Cargo invocation was preceded by `python qa/browser_live/check_memory.py` from the worktree root; Cargo ran serially in the same dev profile/target. / 每次 Cargo 前均运行 memory gate，固定单 job、同 profile/target。
- Earlier Panel checks timed out during compilation; an intermediate `E0382` from the added `aria-pressed` closure was fixed by removing that unnecessary attribute. Final compilation passed. / 早先超时和中途编译错误均披露，不冒充首次全绿。
- Protocol RED/GREEN regressions now prevent empty, partial or duplicate phases from passing and prevent a blocked phase from hiding cleanup ambiguity. Run identifiers use the full 64-hex digest.
- Provenance regressions exposed that arbitrary `kind/reason` text could enter the ledger. The boundary now accepts only the implemented fixed source-map/hash labels and rejects raw path/token text before storage. / provenance 字段已限制为已实现的固定标签；先 RED 再 GREEN，拒绝将路径或 token 文本写入台账。
- Changed Rust files passed targeted `rustfmt --check`; `python qa/browser_live/check_docs.py` and `git diff --check` passed. / 改动 Rust 文件格式、文档检查与 diff 空白检查通过。
- Fixture behavior is verified through the independent built-helper Node WS/HTTP tests rather than a monolithic Core lib-test or a new public test shim. / fixture 使用独立 helper 的真实 WS/HTTP 测试，未增加 public test shim 或重跑 monolithic Core 配置。
- Commits use per-invocation `core.hooksPath=/dev/null` because the existing shared formatting hook is blocked by unrelated baseline files; targeted changed-file formatting passes, and no global hook configuration was changed. / 沿用单次提交绕过既有基线 hook 阻塞，未修改全局 hook 配置；改动文件格式已单独验证。

Ignored local evidence / ignored 本地证据: `.superpowers/sdd/2026-11-01-panel-native-project-context-acceptance/`. It contains compiler logs/status and a redacted preflight report; it does not contain native trace, protected persistence or cleanup proof. / 其中编译日志与脱敏预检不能升级为 native 证据。

## Independent review / 独立审查

The fresh read-only whole-branch reviewer `del_mvxlbhy5_sr3x` returned three findings and ended with **SIGTERM**. This is a substantive written review, not a clean-exit review. It ran no tests. The earlier focused reviewer `del_mvxkn3my_zrsk` also ended SIGTERM and produced no usable findings.

完整分支 fresh reviewer 有书面结论，但进程 SIGTERM，不能称 clean review；另一个 focused reviewer 没有可用结论。三项处理如下：

- **HIGH, open / 未完成**: missing trusted RPC receipts. Kept as an explicit acceptance gap; no native PASS or substitute receipts.
- **MEDIUM, fixed / 已修复**: revealing an already-open Projects menu toggled it closed. Added meaningful failing coverage, then fixed and passed the driver regressions.
- **LOW, fixed / 已修复**: asset observation established pairing bindings. Binding now requires explicit `confirm_pairing`; real Gateway smoke proves an earlier asset yields no receipt/binding and a later confirmed asset yields a valid receipt.

## Not done / 未做与未验证

No real Windows shell launch; protected OS save/load/delete; same-device native reload/relaunch; native trace/monitor handshake; native A/B/Global, cross-user, delayed-result, endpoint/lease or revocation phase; exact OS absence proof; full fresh shell/WASM/reaper provenance; full workspace tests or monolithic Core lib-test. No daily daemon/default release/production credential changes, no old connection-file read, no merge and no push.

未做：真实 native 运行、OS protected persistence 与缺失验证、native 全矩阵、完整 provenance、全 workspace/monolithic tests。隔离分支保留；日常 daemon、默认 release、生产 credential 与旧连接文件未动，未 merge、未 push。只有 native 全链路与隔离/撤销、provenance、cleanup 均有真实证据时，本阶段才能完成。
