# Phase10 Baseline Receipt
# Phase10 基线收据

**Date / 日期:** 2026-11-18
**Purpose / 目的:** Record the exact local starting point before Phase10 implementation. This is a baseline, not an acceptance receipt. / 记录 Phase10 实施前的本地起点；不是验收收据。

## Git identity / Git 身份

- Repository root / 仓库根目录: `/Volumes/TBU4/Workspace/Aleph`
- Branch / 分支: `main`
- HEAD: `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7`
- Remote comparison / 远端对照: `main` matched `origin/main` at baseline.
- Worktree / 工作树: clean before these three new documents; no additional worktree was present during the baseline check.
- Protected history rule / 历史保护: no reset, pull, `origin/dev` rebase, worktree deletion, branch deletion, or evidence deletion was performed.

## Phase9 identity distinction / Phase9 身份区分

The original Phase9 implementation commit is `42764c43d4be5bbe26094a84d1ce9495f08a6dca`; the original plan-document commit is `81e8ceda2bfde52d1c9ae96cabc6c2967cab9953`. The original `runtime-proof` ref was restored non-destructively at `81e8ceda2bfde52d1c9ae96cabc6c2967cab9953`.

The current `main` history does not contain those exact original SHA identities because the history was rebased. Equivalent rewritten commits are `6e3004985` and `0e4e1d4eb`. The original objects remain reachable through the restored ref/object database. Evidence must report “original identity” and “current rewritten identity” separately; an exact ancestry check against the original SHA is not a PASS for current `main`.

## Accepted Phase9 anchors / Phase9 已验收锚点

- NSH revision: `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`
- Cargo.lock SHA-256: `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`
- Acceptance JSON SHA-256: `8714d91fb95a07a95f0cf77f921c1917ca4451de5f31d79db7c9f72bd4e4f181`
- Default WASM SHA-256: `9f708c647e481cb1e1a53947ec8a6c0694895d6f10f9409ba46732e8db367cb8`
- Immutable auditor SHA-256: `1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94`
- Accepted source/default/review inventory: 52 source entries, nine default assets, 30 archived review reports.
- Accepted matrix: `isolated-matrix-40-20261118T185504`, exact Tauri 40 then NSH 40, 10 warmups plus 30 measured per side, 6,400 correlated events and 7,002 resource samples.
- Accepted core evidence: 54 retained terminal invocations; `22,402 passed / 0 failed / 20 existing ignored / 656.84s`; independent adopted resolver run `1,572` entries and `22,402 / 0 / 20 / 661.14s`.

Phase10 read-only reconciliation / Phase10 只读复核:

- All 52 manifest inputs exist. `HEAD` differs from the historical source manifest only at `src/utils/paths.rs`, where upstream `b90b1a81c` changed skill-root resolution. The original and rebased Phase9 patches for that file have the same stable patch ID, `1d2a64a1f4699ad3dae0c607d722587f6f646c91`.
- After archiving the Phase10 QA witness, `qa/nsh_panel_driver.js` returned to its original bytes. `desktop/nsh-shell/src/main.rs` retains only the resident-close option. The reconciliation above found all nine default assets matching `HEAD` and their accepted hashes.
- The eight referenced integrity artifacts and all 30 archived review reports match their recorded digests. The accepted and prior non-accepted matrix directories still exist locally. This verifies the named artifacts, not a fresh run of the matrix or a file-by-file audit of all historical rounds.
- 52 个源码输入均在；上游 rebase 改动和本轮 QA 修改分别记录。九个默认资产、八个完整性引用及 30 份归档评审哈希相符；未重跑历史矩阵。

## Phase9 boundary carried forward / Phase9 边界延续

Phase9 accepted an existing Panel hosted by an NSH 0.5 debug shell. Its public `quit_app()` proof covers explicit application exit, engine stop, kernel exit, and observed member settlement. It does not prove the native user WindowClose callback, resident hide/reopen, tray, single instance, hotkey, deep-link delivery, notification permission/click, or window-state restoration.

The default WASM QA seams remain absent from the canonical default asset: `__alephPanelQaIngress`, `__alephPanelQaOnReconnecting`, and `__alephPanelQaHistoryApplied` must not be introduced by Phase10.

The external managed Gateway must remain alive after shell quit. The shell must not own `~/.aleph/data/aleph.lock`, vault/session/memory storage, or a second server process.

## Current source state / 当前源码状态

- `desktop/nsh-shell/src/main.rs` mounts `shell_host`, the debug QA clock, one `web_view(url)`, and QA message validation. `close-requested` currently calls `nsh_platform_macos::quit_app()`.
- `desktop/nsh-shell/src/shell_host.rs` validates default/QA loopback URL and paired 32-byte lowercase-hex nonce.
- `desktop/nsh-shell/tests/nsh_v05_window_compat.rs` is compile/API coverage, not native lifecycle proof.
- `desktop/nsh-shell/Cargo.toml` pins all four NSH crates to the same v0.5 revision above.
- `desktop/nsh-shell/src/{connection,session,runtime,supervisor}.rs` and `components/` are old unmounted paths and remain unmounted.
- `desktop/shell/src/{tray,hotkey,deeplink,notify}.rs` is behavior reference only; Tauri plugin/daemon ownership is not portable to NSH.

## Phase10 pre-implementation status / Phase10 实施前状态

| Item | Status | Evidence boundary |
| --- | --- | --- |
| Policy/docs read | PASS | Required root, security, tool, desktop, Phase9 and Phase10 documents read before edits |
| Boundary design | FORK CHECK AND TARGETED SPIKE PASS; SPEC BLOCK | Initial fork `6bbf9c96` was not ready per independent QUALITY. SPEC blocked the first follow-up. A later native fixture covers observable startup overflow and a host-origin message, not production Panel delivery. |
| Public `run_app` user-close callback | FORK IMPLEMENTED, ALEPH NOT CONSUMING | The maintained fork adds `AppEvent::WindowCloseRequested` through `AppOptions::event_sink`; Aleph remains on the pinned upstream dependency pending boundary review and production Panel delivery. |
| `keep_alive_on_close` resident behavior | REAL PANEL LIFECYCLE SCOPED PASS | Two native close/reopen/focus cycles retained the same process, window and observed Panel document. A separate LaunchServices run quit while hidden with kernel exit 0 and settled all observed coalition members. Only an isolated Gateway was tested. |
| Host-to-Panel ingress | COMPILE SPIKE PASS, QA-ONLY RUNTIME DELIVERY PASS | `element_ref()` + `Element::element_ref()` compile against pinned NSH; the isolated native witness captured Rust `ElementRef::post_message` -> WebKit `message` -> existing QA `on_message` with matching nonce/run id. Independent receipt: `docs/superpowers/runs/2026-11-18-phase10-webview-delivery-receipt.json` | Production ingress remains unapproved; queue/error/ordering/teardown contract is open |
| Tray/hotkey/notification/state host callbacks | FORK IMPLEMENTED, ALEPH NOT CONSUMING | Fork emits typed owned events from existing NSH owners; upstream pinned v0.5 remains private and Aleph has no production callback consumer |
| Production Rust changes | PINNED RESIDENT OPTION ONLY | Aleph enables the public resident option. The WebView message witness remains QA-only; the fork is not wired into Aleph. |
| G1-G7 acceptance | OPEN | Partial fixture results only; no gate is accepted. |

### Current detached candidate addendum / 当前分离候选补充

The table above is the historical pre-implementation snapshot. The detached candidate at `/Volumes/TBU4/Workspace/aleph-phase10-local` now pins the seven NSH packages to local bundle commit `644a31f0cd1cc35855b6241aedec90a2eda9f222`; the main worktree remains on the original `f4d68f7f` pin. The candidate contains a production Panel `ShellIntentListener` mounted under the existing Router, document-bound `ShellNavigation::Probe/Navigate`, and Core notification forwarding through the existing `DashboardState` subscription. Current candidate verification includes: direct-Git compatibility 4+1 tests, fork native host-boundary fixture, Panel 412 protocol tests, Panel 1568 library tests, G2 single-instance, G4 four-route dispatch, G6 placement, and shared-asset streaming/tool-card folding. These are candidate and scoped results, not Phase10 acceptance. / 上表是历史上的实施前快照。当前分离候选已将七个 NSH 包锁定到本地 bundle commit `644a31f0cd1cc35855b6241aedec90a2eda9f222`，主工作区仍保留原 `f4d68f7f`。候选在现有 Router 下挂载生产 Panel `ShellIntentListener`，使用文档绑定的 `ShellNavigation::Probe/Navigate`，并通过现有 `DashboardState` 订阅转发 Core 通知。当前候选已验证直接 Git 兼容测试 4+1、fork native host-boundary、Panel 协议 412、Panel 库 1568、G2 单实例、G4 四路、G6 窗口状态及共享资产流式/工具卡折叠；这些仍是候选或 scoped 结果，不是 Phase10 总验收。


### G1 native probes / G1 原生探针

- Pinned NSH compatibility tests: **2 passed**, offline, locked, single job. The debug executable SHA-256 was `b0d430ef13b9ac73f4aa0fee450e6cd1d1dcc407de8ff13bc867d7ce47f0b08e`.
- `.phase10-evidence/g1-native-aogd7edi/`: an actual Accessibility close-button action hid window `3595` and left PID `42880` resident with the local HTML fixture. Reopen/focus did not pass and the runner terminated its owned child.
- `.phase10-evidence/g1-native-ls-u4yer5ku/`: `open` failed to launch the isolated bundle with `-10810`. `.phase10-evidence/g1-native-nsw-av6yr7b_/`: the existing NSWorkspace launcher did not return a launch receipt within the probe timeout. No probe process remained after either attempt. Neither run establishes the cause of the reopen/focus failure.
- `.phase10-evidence/g1-native-internal-2aettabq/`: the same debug binary, ad-hoc signed in a fresh local Applications bundle and started directly, passed two Accessibility close-button and LaunchServices reopen/focus cycles. PID `56478` kept CGWindow `3649`; Quit exited `0`; the fixture served one HTML document. The changed staging and launch setup does not isolate the cause of the earlier failure.
- `.phase10-evidence/g1-real-panel-0gameida/`: same launch method against the existing Panel served by an isolated Gateway. Both cycles hid and refocused CGWindow `3658` of PID `57126`. The Panel emitted one `ready`, one authorized handshake and one observed browser time origin. Standard Quit exited `0`; Gateway PID `57108` returned HTTP `200` after host exit, before separate fixture cleanup. Served asset bytes were unchanged. Receipt SHA-256 `f6232e26bd7b845e7cb2678f00e872afedb3446e7486564b9586ecda75a7fa08`.
- `.phase10-evidence/g1-wrapper-settlement-tpn65xv5/`: LaunchServices cold start used the existing Phase9 wrapper and a fresh probe bundle, with capture on a local filesystem. PID `65496` retained CGWindow `3678` through two close/reopen/focus cycles; standard Quit while hidden returned kernel exit `0`. All seven coalition members seen in 51 samples, including GPU, networking and WebContent, were absent after exit. The isolated Gateway returned HTTP `200`. Receipt SHA-256 `f36d1ddda686af9049d06c124beab1e4afc6463149957f46b85039728260abd6`.
- Earlier settlement attempts remain retained: direct execution failed with `incomplete_host_coalition:60211`, direct LaunchServices cold start returned `-10810`, and one wrapped attempt did not observe an authorized Panel. The positive run establishes the measured path, not a diagnosis for every failed launch.
- 真实 Panel 已验证两次关闭、重开同一窗口、隐藏状态退出，以及 51 次采样中观测到的进程退场。隔离 Gateway 仍健康。未验证既有托管 Gateway 或采样间隔内不可见的进程，不等于完整 G1。

## Approved next gate / 已批准的下一闸门

The approved boundary is still not accepted. The detached candidate now has a locally reproducible bundle source, direct-Git integration, bounded checked ingress, a Panel document-bound consumer, and the current candidate tests/fixtures listed above. The main worktree remains on the original dependency pin. Public cloneability of the fork, complete native event/notification proof, managed Gateway survival, final independent review, and G1-G7 acceptance remain open. / 批准边界仍未验收。分离候选现在有本地可复现 bundle 来源、直接 Git 集成、有界 checked ingress、Panel 文档绑定消费者，以及上面列出的当前测试/夹具；主工作区仍保留原依赖 pin。fork 的公共可克隆性、完整原生事件/通知证明、受管 Gateway 存续、最终独立评审和 G1-G7 验收仍未关闭。

GitHub source search: upstream `main` (`1cec09cf8f12f8e73be70f71addc17de981336d5`), four relevant branch tips and 175 readable recent public fork default branches did not expose the candidate event types. One candidate was unreadable. The local candidate SHA was not found in upstream GitHub. No unrelated fork or unreviewed upgrade was selected. / 已实查 GitHub 上游、相关分支和近期公开 fork，未找到匹配候选；未更换依赖。

**Not done / 未做:** No main-worktree dependency switch, public fork URL, follow-up QUALITY approval, OS permission change, managed-Gateway survival proof, release artifact, or G1-G7 acceptance. The detached candidate has production Panel ingress and scoped native evidence, but the earlier pinned-upstream QA witness remains a delivery sub-result and the current candidate is not a whole-run acceptance. / 未切换主工作区依赖、未获得公共 fork URL、未通过后续 QUALITY、未改变 OS 权限、未证明受管 Gateway 存续、未发布，也未验收 G1-G7。分离候选已有生产 Panel 入口和 scoped 原生证据，但早期 pinned-upstream QA witness 仍只是投递子结果，当前候选也不是整轮验收。
