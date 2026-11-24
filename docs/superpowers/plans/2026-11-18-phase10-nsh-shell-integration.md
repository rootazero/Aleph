# Phase10: NSH macOS Shell Integration — Implementation Plan
# Phase10：NSH macOS 桌面壳集成实施计划

> **For agentic workers:** Use `superpowers:subagent-driven-development` only when delegation is authorized, or `superpowers:executing-plans` for direct execution. Track the checkboxes below. Read the restart prompt before implementation.

**Status / 状态:** Completed for the approved local macOS slice under the [2026-11-21 Quit-only amendment](../runs/2026-11-21-phase10-acceptance-amendment.md); see the [closeout](../runs/2026-11-21-phase10-closeout.md). Physical multi-display topology remains explicitly user-deferred and NOT TESTED, not PASS. / 已按修订合同完成本机范围，多屏延期未测。 Rebased on the installed official NSH `0.6.0 (5541637)` and the matching crates.io release. The [0.5 fork checkpoint](../runs/2026-11-19-phase10-local-integration-checkpoint.md) is historical only; its implementation and evidence do not carry forward. Follow the [current native QA ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md) for outstanding gates. Run desktop QA on this Mac, with temporary fixture data and no VM. / 以本机官方 NSH `0.6.0 (5541637)` 和对应发布 crate 为基准；旧补丁与回执仅作历史记录。验收状态见当前本机回执清单；使用临时测试数据，不构建虚拟机。

**Goal / 目标:** Make the NSH macOS Panel host usable as a resident desktop shell: real window close/reopen/quit, tray and single-instance delivery, summon hotkey, deep-link delivery, notifications and window-state restoration, with explicit resource ownership and native evidence. / 在现有 Panel 宿主上补齐可验证的常驻桌面壳能力。

**Architecture / 架构:** Keep Leptos/WASM Panel as the sole business UI, Chat/session state and Gateway transport owner. NSH owns only OS integration and the lifetime of its window/WebView, callbacks and registrations. Use documented release APIs and existing Aleph contracts; keep Core platform independent. / 业务仍在 Panel；NSH 只拥有 OS I/O 与自身资源生命周期。

**Tech Stack / 技术栈:** Rust, official NSH 0.6.0 from crates.io, public host/Bridge APIs, existing Leptos/WASM Panel, deterministic isolated Gateway QA, Python native evidence tools. No new heavy runtime dependency by default.

**Spec / 规格依据:** [Official NSH 0.6.0 boundary](../specs/2026-11-20-phase10-official-nsh.md), [NSH master](../specs/2026-11-05-nsh-desktop-refactor-design.md), [R2 choice A](../runs/2026-11-16-phase9-nsh-r2-boundary.md), [Phase9 accepted contract](../specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md), [Panel parity contract](../specs/2026-11-15-nsh-panel-parity-slice.md), [Phase10 entry brief](../runs/2026-11-18-phase10-entry-brief.md). Task 1 defines the official boundary; the other references retain their original scope. / 任务 1 定义官方接口边界；其他参考仍保留原有适用范围。

**Restart prompt / 重启提示词:** [2026-11-18-phase10-nsh-shell-restart.md](../prompts/2026-11-18-phase10-nsh-shell-restart.md).

## 1. Authority, numbering and design trade-offs / 权威、编号与取舍

1. The NSH master is a historical **DRAFT for v0.4**, with Phases **0–6**, not a Phase10 specification. Read all 393 lines, especially §§2–3, 5, 6 and 8. Phase10 here names the continuation after accepted Phase9; it is not master Phase5 Plugin Runtime, nor a new Capability Core roadmap claim. / 两套阶段编号不能互相充当完成证明。
2. The explicit R2 decision is already made: **choice A, NSH WKWebView hosts the existing Panel**. Do not reopen it or silently replace it with native Chat. The master's native business UI and proposed `interfaces/nsh-desktop` are not this phase's implementation instructions.
3. Phase9 final acceptance and final integrity supersede old `D OPEN/BLOCKED` checkpoints. Preserve those checkpoints and the rejected blocks as history; do not rewrite their verdicts.
4. The proposed Phase10 scope below fills concrete gaps in the master's **Phase2 OS integration**. It does not declare the master's entire Phase2/3/4 done. Phase5 requires its ADR prerequisites; Phase6 requires Phase4 plus the stated two-week dual run. Neither is authorized here.
5. Thin native integration preserves R2 but requires an explicit Panel navigation/intent boundary and real OS tests. Resident behavior makes close different from quit and requires owned registration teardown. Use NSH's documented 0.6.0 contracts and prove their behavior. Any gaps must be reported explicitly; the superseded 0.5 fork is not a fallback. / 使用官方发布版契约，原生行为以实测为准；不回退到旧补丁。
6. This plan is a proposal requested for the next session. Using the companion prompt starts work on this bounded proposal. A constitution change, new daemon ownership model or release trust change still needs a concrete decision before dependent implementation. Routine review fixes inside the agreed scope do not need repeated permission.

## 2. Accepted baseline / 已验收基线

The accepted Phase9 tree is committed as `42764c43d4be5bbe26094a84d1ce9495f08a6dca` (`feat(nsh): close phase9 runtime proof and dependency validation`). The user requested local merge to `main`; on restart verify this commit is an ancestor of the actual main HEAD. The original measurement base/HEAD was `3892804c112fd6e574166a4c03a7c40a77d0b239` on `runtime-proof`; it is a **historical measurement identity**, not the HEAD to reset to. Preserve the integrated Phase9 tree and its original receipt identities.

| Gate | Accepted evidence / 已有证据 | Limit / 限界 |
| --- | --- | --- |
| A | Retained graph: 54 terminal PASS invocations; full Core `22,402 passed / 0 failed / 20 existing ignored / 656.84s` | Do not add nested subprocess summaries to the total |
| B | Adopted real resolver graph: 1,572 entries; separate full Core `22,402 / 0 / 20 / 661.14s` | Not the same run as A |
| C | Existing production Panel, canonical session/agent projection, history race regressions, real queued wire → production dispatch → ChatState | Panel 1,568; Tauri 190; NSH 5 tests at the accepted build |
| D | Entire fresh block `isolated-matrix-40-20261118T185504`: 40 Tauri then 40 NSH, each 10 warmups + 30 measured; all 80 runs PASS | Approved macOS debug slice only |
| E | Final SPEC `1b81b8e5` → distinct-model QUALITY `8377f29f`; final handoff verification PASS | Reviewer notes and limitations remain recorded |

Important identities / 关键身份：

- `Cargo.lock` SHA-256: `12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`.
- NSH Git revision: `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`.
- Final acceptance JSON SHA-256: `8714d91fb95a07a95f0cf77f921c1917ca4451de5f31d79db7c9f72bd4e4f181`.
- Canonical default WASM SHA-256: `9f708c647e481cb1e1a53947ec8a6c0694895d6f10f9409ba46732e8db367cb8`.
- Immutable Phase9 auditor SHA-256: `1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94`.
- 52 accepted source files, nine restored default assets, 30 archived review reports; 6,400 raw correlated events and 7,002 real resource samples in the accepted block.
- 17 complete historical matrix directories: **15 INVALID**, one `174308 VALID / NOT_ACCEPTED`, one `185504 PASS`; all default restorations succeeded. No selected-round splicing.

| Median metric / 中位数 | Tauri | NSH |
| --- | ---: | ---: |
| Startup to first-frame receipt, ms | 5357.951 | 4933.0425 |
| Chat route, ms | 33 | 33 |
| Input to addressed `chat.send` receipt, ms | 198.5 | 198.5 |
| Reconnect repair, ms | 1041.5 | 1022.5 |
| Application quit settlement, ms | 47.679 | 48.921 |
| Peak RSS, MB | 678.1015625 | 640.84375 |
| Maximum live threads | 76 | 68 |

Close p95 was `77.444 → 61.365ms`. All original thresholds passed. The application-quit test uses public `quit_app()` and independent kernel/all-observed-member checks; it **does not prove user WindowClose callbacks**. Phase10 must supply that evidence itself.

## 3. Required reading and source owners / 必读与源码权威

Read in this order; historical plans are a gap checklist, not authority to restore native business UI. / 按顺序阅读，旧计划只用于差距审计。

1. Root `AGENTS.md`; `docs/reference/SUBSYSTEM_ROUTING.md`, `REDLINES.md`, `DEVELOPMENT.md`, `WORKSPACE.md`, `STYLE.md`, `PROCESS_MANAGEMENT.md`, `SECURITY.md`, `TOOL_SYSTEM.md`; relevant subsystem instructions. Read `HARNESS_PHILOSOPHY.md` only if touching that boundary; this phase should not modify the harness.
2. **Entire NSH master:** `docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md`.
3. `docs/superpowers/plans/2026-11-05-nsh-phase0-spike.md`, `2026-11-05-nsh-api-verification.md`, `2026-11-05-nsh-input-limitations.md`; `desktop/nsh-spike/FINDINGS.md`.
4. `docs/superpowers/plans/2026-11-05-nsh-desktop-phase1-protocol-transport.md`, `2026-11-05-nsh-desktop-phase2-connection.md`, `2026-11-05-nsh-desktop-phase3-ui-core.md`, `2026-11-05-nsh-desktop-phase3-technical-debt.md`, `2026-11-05-nsh-desktop-phase4-capability-alignment.md`.
5. `docs/superpowers/plans/2026-11-15-phase8-integration-security-and-nsh-restart.md`; `docs/superpowers/specs/2026-11-15-phase8-integration-security-design.md`; final Phase8 handoff `docs/superpowers/runs/2026-11-16-phase8-main-handoff.md`.
6. Current R2 and accepted Phase9 files in the Spec header; `docs/superpowers/runs/2026-11-16-phase9-closeout.md`, `2026-11-16-phase9-execution-receipt.md`, `2026-11-16-phase9-final-integrity.json`, `2026-11-18-phase9-acceptance.json`, `2026-11-18-phase9-source-manifest.json`, `2026-11-18-phase9-default-manifest.json`, `2026-11-18-phase9-parity-receipt.json`.
7. `qa/phase9_runtime.md`, `qa/nsh_panel_launcher.py`, `qa/nsh_panel_matrix.py`, `qa/nsh_panel_gateway.py`, `qa/nsh_panel_driver.js`, `qa/nsh_panel_parity.py`, `qa/test_phase9_native_meter.py`; use the measurement repair report when a historical failure mechanism matters.

Current source facts to recheck / 当前落点（重启后复核）：

| Path | Actual responsibility / 真实职责 |
| --- | --- |
| `desktop/nsh-shell/src/main.rs` | Only `shell_host` plus debug QA clock is mounted; one existing Panel WebView, 1180×800, `com.aleph.nsh-shell`; QA nonce/run-bound quit |
| `desktop/nsh-shell/src/shell_host.rs` | `HostSettings::from_inputs(url, nonce)`; default `http://127.0.0.1:18790/`; confined debug loopback+nonce validation |
| `desktop/nsh-shell/tests/nsh_window_compat.rs` | Published NSH window and WebView API compile coverage; not proof of user-close semantics |
| `desktop/nsh-shell/src/{connection,session,runtime,supervisor}.rs` | Old unmounted paths; do not mount duplicate Gateway/session state or detached `ensure_ready` by convenience |
| `desktop/nsh-shell/src/components/` | Old unmounted native business UI; keep unmounted |
| `desktop/shell/src/{main,tray,hotkey,deeplink,notify}.rs` | Tauri behavior reference; its `Supervisor` is in `main.rs`, not a separate `supervisor.rs`; platform wrappers are not Core policy |
| `interfaces/webchat/src/{app,context}.rs` | Existing Panel state/transport and route owner; maintain its attempt/epoch guards |
| `interfaces/webchat/src/platform/wide/views/chat/events.rs` | Production event projection, including queue lifecycle; lazy i18n boundary is intentional |
| `interfaces/webchat/src/components/chat_sidebar.rs` | History hydration race protection and QA history witness |
| `shared/ui_logic/src/connection/wasm.rs` | Actual Panel WASM transport; a new native `shared/client` loop would duplicate it |

`desktop/nsh-shell/src/tray.rs`, `hotkey.rs`, `deeplink.rs`, `window_state.rs` and `notifications.rs` are **proposed new files, not existing implementations**. Keep the final module split small and based on proven ownership boundaries.

## 4. Global constraints / 全局约束

- Work from the user's **local main**, not `origin/dev`. Resolve the current Git root; documents use relative paths. Do not pull/reset main, delete branches/worktrees/evidence, or overwrite unknown edits. Use short branch names if a later user requests a branch.
- Never read, modify or stage `.claude/settings.json`, root `config.lock` or root `config.toml`. Do not change `AGENTS.md`/`CLAUDE.md` or architectural redlines as an implementation shortcut. `Cargo.lock` is allowed, but keep the accepted pin unless an actual resolver change is justified and reviewed.
- **Before source exploration, run Graphify**, then targeted reads/searches. Preserve this instruction verbatim in any authorized child-agent brief. Inspect current files before editing; the prior session had concurrent/shared edits.
- One Cargo owner, no parallel Cargo. Before **every** Cargo invocation, run `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`; continue only on ALLOW. Use `--offline --locked --jobs 1`, with `CARGO_TARGET_DIR` pointing at a new `.phase10-target`. Do not overwrite `.phase9-target` binaries that bind old evidence.
- Read relevant testing/debugging/review skills before changes. Prefer actual native/component state transitions over global mocks or implementation-mirroring tests. Record actual RED → GREEN for substantive regressions; missing helper tests alone do not prove a causal bug.
- Independent **SPEC → QUALITY**, in that order, for the implemented scope; use a different Pi model for the second opinion. Delegate only with authorization. No Codex/Claude/Gemini CLI fallback. DeepSeek is **Flash only**.
- New externally composable behavior follows the existing Capability/tool contracts. Inspect the current canonical owners before proposing a new callable surface. Do not add a second registry, lease map, channel authority, tool metadata authority or business reducer.
- Keep disabled features resource-free; own callbacks, registrations and workers. No detached threads or implicit global Tokio runtime for optional OS features. R1/R4/R6 remain binding.
- Do not change the Phase9 reducer or thresholds, relax ownership, use whole-machine deltas, run as root, use private macOS APIs or change OS security policy. Do not ship QA ingress/callbacks in default WASM.
- `just stage-shell-placeholders` can replace real sidecars with zero-byte placeholders. Snapshot and restore any owned binary it touches; never use placeholders as full-package proof. Debug RustEmbed reads the **current** `interfaces/webchat/dist`, so a copied Gateway binary does not freeze Panel assets.
- Preserve raw evidence and review originals byte-for-byte, including historical whitespace. Source/new-document whitespace checks are separate from diagnostics in retained logs; do not rewrite hashed receipts to silence `git diff --check`.
- Communicate in Chinese; code comments in English; new documents bilingual. Report changes, trade-offs, actual checks and skipped validations explicitly.

## 5. Scope and acceptance contract / 范围与验收合同

The spec created in Task 1 must resolve these observable cases without adding a native business UI. / Task 1 的具体规格须逐项落实这些行为。

| Gate | Required behavior / 必须实现的行为 | Required evidence / 必须提供的证据 |
| --- | --- | --- |
| G1 Window | User close enters the agreed resident-hidden state; reopen/focus reuses the existing Panel; explicit Quit settles shell-owned resources; close/quit causes remain distinguishable | Actual public native window callbacks, close → reopen → quit, repeated lifecycle, no duplicate receiver/window |
| G2 Tray & instance | Tray Show/Quit use the same lifecycle owner; a second launch forwards only a validated launch intent and exits, without a second resident host | Two real same-user launches; one resident window owner; primary/secondary shutdown and registration cleanup |
| G3 Hotkey | Summon hides/shows/focuses the existing window; conflict/registration errors are visible; disabled mode has no registration/worker | Native hotkey delivery, conflict case, teardown/re-registration |
| G4 Deep link | Existing supported URI grammar is validated, then delivered once to the existing Panel navigation owner; business state stays in Panel | Cold/warm delivery, second-instance delivery, invalid/unsupported input rejection and unchanged projection |
| G5 Notification | Existing notification intent is shown through the host boundary; click focuses the current Panel and routes through its existing owner | Real native notification + callback where OS permission permits; denied permission is reported, never bypassed |
| G6 Window state | Restore supported normal-frame/visibility state; reject invalid/off-screen state safely; presentation state does not become session authority | Relaunch/restore and invalid/off-screen checks using actual public host events. Physical multi-display topology is user-deferred for this Phase10 run and remains NOT TESTED. |
| G7 Lifecycle & regression | Quit joins/releases every new shell-owned worker/receiver/registration; external managed Gateway is untouched; existing Chat/Artifacts/reconnect guards survive | Native exit + observed-member settlement, registrations gone, controlled managed Gateway still healthy, scoped tests/default asset check and fresh comparable runtime proof |

If G1's public API feasibility fails, produce a concrete supported alternative and obtain the needed decision; do not advertise a tray app with unproved close semantics. Missing OS permission is a scoped validation blocker, not evidence that G5 passed.

Out of scope / 不在本期：release installer/notarization/auto-update installation, switching the default desktop, full embedded-core bundle ownership, detached daemon takeover, Core execution redesign, installed-service `SimulatedEngine` transcript repair, new native Chat/Artifacts/Settings business UI, Plugin Runtime, Swift camera/voice/screenshot features, i18n migration, Windows/Linux runtime acceptance, Tauri/WebChat/workspace removal and two-week rollout completion. No remote push/release is implied.

## 6. Implementation tasks / 实施任务

### Task 1 — Reconcile main, prove APIs, freeze the Phase10 spec / 复核 main、验证 API 并形成规格

**Files:** Read all required references; create `docs/superpowers/specs/2026-11-18-phase10-nsh-shell-integration.md`, `docs/superpowers/runs/2026-11-18-phase10-baseline.md`, `docs/superpowers/runs/2026-11-18-phase10-nsh-api-inventory.md`. Extend the date only if the next session intentionally chooses its actual execution date.

**Interfaces:** Integrated main tree + accepted manifests → explicit G1–G7 cases, real source owners, public pinned API list and a master Phase0–6 DONE/PARTIAL/NOT_PROVEN/DEFERRED table.

- [x] Reconcile local `main` with the accepted Phase9 baseline. The [baseline](../runs/2026-11-18-phase10-baseline.md) distinguishes the original `runtime-proof` ref from the rebased Phase9 ancestor of `main`; the [official checkpoint](../runs/2026-11-20-phase10-official-nsh-checkpoint.md) identifies the current candidate. / 原 Phase9 证据与主分支上 rebase 后的提交分开核对，不以历史 HEAD 覆盖当前内容。
- [x] Map the master Phase2 items to the documented shell and Panel owners; updater and full daemon-supervisor parity remain deferred. See the [master handback](../runs/2026-11-20-phase10-official-nsh-checkpoint.md#master-phase-2-handback--总设计-phase-2-回看). / 明确原总设计已实现与未交付的项目，不将 updater 或服务监管标记为完成。
- [x] Query Graphify and inspect official NSH 0.6.0 public APIs. The [official spec](../specs/2026-11-20-phase10-official-nsh.md) and current gate receipts record the selected APIs, native behavior and limitations. / 先查 Graphify，再核对官方版本的公开 API 与原生行为。
- [x] Define native close/hide/reopen/Quit and ownership in the [official spec](../specs/2026-11-20-phase10-official-nsh.md); the managed Gateway is external and the native intent is validated before Panel delivery. / 区分关闭、隐藏、重开和退出；不把独立 Gateway 归给桌面壳。
- [x] Compile Aleph's shell with the official crates.io `0.6.0` lock. Verify the installed `nsh 0.6.0` macOS host can send and receive through a real WKWebView. This is a standalone probe, not a production Panel receipt. / 官方锁定编译和 WKWebView 原生往返已验证，不计作生产 Panel 验收。
- [x] Specify validation, bounded admission, ordering, document identity, observed delivery and teardown using public APIs only. The revised proposal passed independent SPEC (`de87c050-f32c-4382-ab48-3f0b58b998de`); Task 8 still requires final implementation SPEC and QUALITY. / 官方版方案已通过独立 SPEC；最终实现仍须按任务 8 复审。

### Task 2 — Implement real window lifecycle / 实现真实窗口生命周期

**Files:** Modify `desktop/nsh-shell/src/main.rs`, `desktop/nsh-shell/src/shell_host.rs`; add `desktop/nsh-shell/src/lifecycle.rs` only if it names a coherent reusable lifecycle owner; extend `desktop/nsh-shell/tests/nsh_window_compat.rs` and add focused lifecycle/native QA fixtures as needed.

**Interfaces:** Official NSH 0.6.0 window and WebView APIs → one owned host lifecycle. `HostSettings` continues to validate launch input; no session/Gateway transport moves into it.

- [x] Verify the 0.6.0 host can hide the real Panel on close and refocus the same window. The [current local receipt](../../../.phase10-evidence/g1-official-current-e3w84dqy/receipt.json) records two targeted LaunchServices cycles, real `/chat`, hidden Quit exit 0, observed coalition settlement and the same isolated Gateway. The earlier `/usr/bin/open` attempt remains a rejected approach. / 当前版真实 Panel 定向重开两次均回到原窗口，隐藏后退出为 0，观测进程均结算，隔离 Gateway 不变。保留此前普通 `open` 的失败记录。
- [x] Consume only the [reviewed official callback and ingress contract](../specs/2026-11-20-phase10-official-nsh.md). The [targeted route receipt](../../../.phase10-evidence/g4-official-panel-iorxe2bl/receipt.json) exercises the real external Panel. / 仅依赖已审阅的官方公开 API，已有真实 Panel 回执。
- [x] Implement resident close/reopen/focus and explicit quit through one owner. Preserve QA nonce/run checks and the real shutdown path. / 关窗常驻与显式退出由同一原生窗口负责，QA 使用真实退出路径。
- [x] Cover repeated native close/reopen, hidden Quit, invalid configuration and observed callback-owner settlement. The current local receipt above verifies real host and WebKit departures; the [startup receipt](../../../.phase10-evidence/official-invalid-startup.json) verifies that invalid configuration is rejected before a native window is started. / 覆盖实际关窗重开、隐藏退出与启动参数拒绝，结算核对原生进程，不只检查 `run_app` 返回。
- [x] Review the observed lifecycle: the same window and Panel are retained; explicit Quit exits normally while the Gateway stays healthy. / 原窗口与 Panel 保留；显式退出正常，独立 Gateway 不受影响。

### Task 3 — Tray and single-instance intent delivery / 托盘与单实例意图投递

**Files:** Modify `desktop/nsh-shell/src/main.rs`; proposed `desktop/nsh-shell/src/tray.rs` and `desktop/nsh-shell/src/instance.rs`; add focused actual-process tests/native QA.

**Interfaces:** Official NSH tray and LaunchServices delivery → Task 2 lifecycle owner and existing Panel route owner. On macOS, `next_second_instance` does not deliver events; check native reactivation and claimed URL delivery before adding any instance transport. / 先验证系统原生重开和已声明 URL 的投递；macOS 不发 `next_second_instance` 事件。

- [x] Connect tray Show/Quit to the real Task 2 behavior with root-owned NSH callbacks. [Native tray receipt](../../../.phase10-evidence/g2-tray-d6pwnub_/receipt.json). / 托盘用 NSH 根组件回调显示原窗口并真实退出。
- [x] Verify two normal packaged launch requests preserve one resident owner, as macOS LaunchServices reactivates the original process rather than starting a forwarding child. The [resident launch receipt](../../../.phase10-evidence/g1-official-current-e3w84dqy/receipt.json) verifies original PID/window identity; the [default URL receipt](../../../.phase10-evidence/g4-default-dispatch-lmd3unzg/receipt.json) verifies the warm intent reaches that owner. A forced second process is not equivalent and was not tested. / 普通打包启动请求由系统交给同一进程窗口，没有需要退出的转发子进程；不外推到强制新实例。
- [x] Verify hidden-owner activation, normal exits followed by fresh launches, and rejection without route effects. The [default URL receipt](../../../.phase10-evidence/g4-default-dispatch-lmd3unzg/receipt.json) records distinct cold and next launch processes, same-window warm activation, and invalid URL rejection. / 核对隐藏唤回、退出后重新启动及非法意图不导航。
- [x] Review ownership: no Aleph singleton socket, lock, or detached fallback is added. Hotkey conflict and release are rechecked through real registrations. / 不新增单例 socket、锁或 fallback daemon；以真实注册验证冲突与释放。

### Task 4 — Hotkey and deep-link host boundary / 热键与 deeplink 边界

**Files:** Proposed `desktop/nsh-shell/src/hotkey.rs`, `desktop/nsh-shell/src/deeplink.rs`; read `desktop/shell/src/hotkey.rs`, `desktop/shell/src/deeplink.rs`, `shared/protocol/src/paths.rs` and existing Panel route handling before selecting exact modified paths. Add Panel intent handling only if a canonical boundary is missing.

**Interfaces:** Reviewed public NSH OS callback → validated host action or existing Panel navigation through the bounded ingress. Reuse canonical URI grammar; do not create an independent native session/tool dispatcher.

- [x] Verify real summon/focus/toggle, disabled mode, and refuse/release/re-register on the current build. The [current conflict receipt](../../../.phase10-evidence/g3-official-conflict-6dkacdqz/receipt.json) records three clean exits and unchanged Gateway identity and health. / 当前构建验证唤回、聚焦后隐藏、禁用、冲突及退出后重新注册。
- [x] Exercise targeted warm, cold, and repeat LaunchServices URL delivery on the signed candidate. The [native route receipt](../../../.phase10-evidence/g4-official-panel-iorxe2bl/receipt.json) shows warm delivery to the existing PID and rejection of unsupported query arguments. The [reload receipt](../../../.phase10-evidence/g4-official-reload.T89qTsUR/receipt.json) shows delivery to the new document only. / 定向冷启动、同进程热交付、非法参数拒绝与跨重载交接有真机回执。
- [x] Verify unqualified LaunchServices dispatch to NSH. The [native receipt](../../../.phase10-evidence/g4-default-dispatch-lmd3unzg/receipt.json) covers authorized temporary selection of the signed candidate, cold launch, invalid input, warm focus on the original window, and restoration of `/Applications/Aleph Panel.app`. / 签名候选包通过真实默认调度；测试后已恢复原默认处理器。
- [x] Record the public NSH callback, lifetime, and acknowledgement contract in the [official spec](../specs/2026-11-20-phase10-official-nsh.md). / 公开回调、生命周期与回执边界已记录。
- [x] Verify the bridge is installed only in the trusted top-level document. The [real WKWebView origin probe](../../../.phase10-evidence/g4-origin-usqgd91o/receipt.json) observes no Aleph bridge in a same-origin iframe or a top-level document on a different loopback origin. This is a bridge installation witness, not an additional Panel Router run. / 真实 WKWebView 观测到同源子帧与异源顶层均无 Aleph 桥；这不是额外的 Router 路由用例。
- [x] Reuse the canonical Panel Router and Core notification intent; no configurable host action, RPC, or registry was added. / 复用 Router 与 Core 通知意图，不新增可配置动作、RPC 或注册表。
- [x] Review document and lifecycle boundaries. The [reload receipt](../../../.phase10-evidence/g4-official-reload.T89qTsUR/receipt.json) records zero actions in the old document and one in the new document; the source reviews confirm one native root and no second Gateway client. / 重载时旧文档无动作、新文档恰一次；无额外 Gateway 客户端。

### Task 5 — Notifications and window state / 通知与窗口状态

**Files:** Proposed `desktop/nsh-shell/src/notifications.rs`, `desktop/nsh-shell/src/window_state.rs`; existing Tauri notification/window-state behavior and public pinned APIs; scoped tests and native evidence.

**Interfaces:** Existing notification intent and OS presentation state → owned native adapter and current Panel route owner. Persist only host presentation settings in the canonical allowed settings location.

- [x] Verify native notification permission, presentation and click. The signed QA bundle received and displayed a real Core notification, and the macOS Notification Center card action focused the same window and reached `/chat`. Earlier denied and muted runs are retained in the [evidence ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md). / 专用签名包通过实际系统授权、展示和通知中心点击；原窗口重新聚焦并进入 `/chat`。此前权限拒绝和静默结果仍保留。
- [x] Exercise normal geometry restore, off-screen clamping and corrupt-state fallback with the [current native placement receipt](../../../.phase10-evidence/g6-actual-ikue8zka/receipt.json). / 已在当前壳的原生窗口验证正常恢复、离屏纠正与损坏回退。
- [ ] Verify placement across a physical display topology change (user-deferred for this Phase10 run; NOT TESTED). / 验证跨物理显示器拓扑恢复（用户暂缓，本轮未测试）。
- [x] Verify the existing disabled summon-hotkey branch makes no shortcut registration, and real Quit settles observed host/WebKit members and releases registrations. Root-owned URL/notification listeners are not detached Aleph workers. Notifications have OS permission checks, not a product disable toggle; no zero-resource notification-disable claim is made. See the [closeout](../runs/2026-11-21-phase10-closeout.md). / 仅热键有禁用模式；通知未定义功能开关，不以拒绝权限冒充零资源禁用。
- [x] Self-review: the host stores no business/session projection and writes placement only through NSH-owned storage. No protected root configuration was changed. G5 requires the joined native action receipt, not permission or admission alone. / 已按源码和实际通知点击回执校对，未用授权代替交付证明。

### Task 6 — Owned resource and managed-server closure / 资源与外部服务边界闭合

**Files:** The new host modules; `qa/phase10_nsh_shell.py` and `qa/test_phase10_nsh_shell.py` are proposed QA entrypoints; reuse proven lower-level `qa/nsh_panel_launcher.py` capture facilities where applicable without changing Phase9 semantics.

**Interfaces:** Host registrations/workers/window + externally managed loopback Gateway → independently observable terminal facts.

- [x] Exercise invalid startup, normal packaged reactivation, user close/reopen and Quit while hidden with an active model request. The [invalid startup receipt](../../../.phase10-evidence/official-invalid-startup.json) rejects configuration before starting the native window. The [resized lifecycle receipt](../../../.phase10-evidence/g1-official-resized-6_7rpfra/receipt.json) verifies two same-PID reopens after the LaunchServices client has returned. The [busy Quit receipt](../../../.phase10-evidence/g7-official-busy-oswcl4oj/receipt.json) gives kernel exit `0` and observed-member settlement while the Provider is still blocked. Under supported normal macOS launches no forwarding child or secondary owner exists; forced new processes are outside the contract. / 配置拒绝发生在原生窗口启动前；LaunchServices 定向请求返回后，原进程仍可重开同一缩小窗口；模型请求未结束时隐藏退出，核验真实退出和进程结算。受支持的打包启动没有转发子进程，不覆盖强制新启动。
- [x] Verify native kernel exit and settlement of the observed host/WebKit members in the [current managed-Gateway witness](../../../.phase10-evidence/g7-official-managed-9mt2xujl/receipt.json) and both full debug matrices. This does not cover processes that never appeared in an ownership observation. / 当前托管 Gateway 用例与两组矩阵均验证了真实退出码和已观测进程终态；不外推到未观察到的进程。
- [x] Prove a pre-existing, independently managed Gateway remains healthy after shell quit. The [temporary launchd witness](../../../.phase10-evidence/g7-official-managed-9mt2xujl/receipt.json) verifies the same listener, PID and start time, matching assets and HTTP 200 after owned-member settlement. No installed service is touched. / 临时 `launchd` 用例证明同一监听进程在壳退出并结算后继续服务；不触碰已安装服务。
- [x] Record raw command/argv, source/build/asset/lock identities, process identity and terminal checks in retained receipts and the [candidate integrity index](../runs/2026-11-20-phase10-official-candidate-integrity.json). The [collector checks](../runs/2026-11-20-phase10-official-nsh-checkpoint.md#candidate-integrity-and-collector-checks--候选完整性与采集器验证) exercise rejection and failure paths. / 记录命令、构建和进程身份、资产校验与终态证据；运行退出判据的失败路径回归。
- [x] Check that the collector validates final attribution, rechecks PID/start before failure-cleanup signals and rejects forced cleanup as success. Identity rechecks are not an atomic signal primitive. / 复核终态归属、失败清理前的 PID/启动身份及强制清理拒绝判据；不声称校对与发信号是原子操作。

### Task 7 — Regression, performance and release-safe default / 回归、性能与默认资产

**Files:** `desktop/nsh-shell/`, affected Panel/shared modules, new Phase10 QA, `qa/phase9_runtime.md` as a contract reference; create `docs/superpowers/runs/2026-11-18-phase10-test-matrix.json` and source/default manifests.

**Interfaces:** Current implementation → scoped checks and fresh native acceptance. Historical Phase9 PASS is a baseline, not a new build result.

- [x] Run relevant memory-gated, offline/locked/jobs1 checks. The [checkpoint](../runs/2026-11-20-phase10-official-nsh-checkpoint.md) records NSH 4+2, Panel 1,568, focused protocol, WASM and debug build. This is not a workspace-wide test claim. / 已完成相关包和 WASM 的重点验证，未声称全工作区通过。
- [x] Build both QA and canonical default WASM serially. Local build manifest: `.phase10-evidence/official-canonical.mYMCixUf/sha256.json`; observed HTTP assets are recorded separately in the targeted route and matrix receipts. The default variant excludes the QA ingress, reconnect and history witnesses. / 已串行构建并校验两种资产，实际 HTTP 内容另在回执中校对。
- [x] Verify shared Chat streaming and reasoning disclosure at normal and smaller native window sizes. The [evidence ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md) records both surfaces at their initial size and after native resize to `720 × 620` points. / 两端正常尺寸和缩至 `720 × 620` 点后的流输出、结果与折叠可用性均有回执；不证明缩放时的流延迟。
- [x] Recheck resident close, same-window focus and hidden Quit after a native resize to `720 × 620` points. The [resized lifecycle receipt](../../../.phase10-evidence/g1-official-resized-6_7rpfra/receipt.json) covers G1/G2 at the smaller size. This does not exercise changing physical displays. / 缩至 `720 × 620` 点后实测关窗、唤回原窗口及隐藏退出；不代替实际显示器变化验证。
- [x] Execute three complete Phase9-style debug comparison blocks on the official candidate: Tauri 40 then NSH 40, fresh Gateway/provider/session every round, 10 warmups + 30 measured, 100ms real owned-resource sampling and the unchanged reducer. All three are retained in the [evidence ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md); completion does not imply acceptance. A separate `opt-level=3` 40+40 diagnostic is also retained as a distinct profile and returned `NOT_ACCEPTED`. / 官方候选版三组原始 debug 矩阵均各 40+40 轮；另有一组独立 `opt-level=3` 诊断，均保留且未验收。
- [x] Verify the Gateway input fix in a fourth full debug comparison. The shared Gateway no longer blocks fast RPCs on `system.info`; the focused dispatcher suite passed 9/9, and the original input gate passed in the [fourth block](../runs/2026-11-20-phase10-official-nsh-checkpoint.md#formal-g7-with-the-gateway-input-fix--带-gateway-输入修复的正式-g7). Native Quit median and p95 still failed. / 第四组正式矩阵验证了 Gateway 输入修复，定向测试 9/9 通过，原输入判据通过；原生退出中位数和 p95 仍超限。
- [x] Test early tray release in a complete fifth debug comparison, after starting over from an invalid sampler block. The [complete block](../runs/2026-11-20-phase10-official-nsh-checkpoint.md#quit-controls-and-embedding-comparison--退出入口与嵌入模式对照) was valid but rejected on Quit median alone (`1.252×`). Revert the candidate and retain both records. / 第五组正式矩阵从首轮重跑，只有退出中位数超限；撤回托盘候选并保留两份记录。
- [x] Re-evaluate the unchanged final candidate's complete retained comparison under the explicitly approved [Quit-only amendment](../runs/2026-11-21-phase10-acceptance-amendment.md): only `close_ms` median and p95 limits are ≤1.30×. All other latency limits stay median ≤1.10× and p95 ≤1.20×, RSS ≤1.10×, thread median delta ≤2; p95 rank remains 29. The [separate assessment](../runs/2026-11-21-phase10-acceptance-amendment.json) passes; the original matrix remains VALID / NOT_ACCEPTED. Preserve every block; no new run, sample splicing, best-of selection or altered collector/reducer. Future invalid blocks restart from zero; rejected valid blocks are not rerun unchanged for a favorable result. / 本次只按两次明确授权重评完整既有数据，不改原失败回执，不声称提速。
- [x] Self-review: existing source/asset/terminal audit reconstructs all 80 natural exits, settlements and unforced Gateway cleanups and 320 exact HTTP asset files. Tracked defaults are restored; no Cargo/native run was started for this amendment. / 本次只读核验，不引入并行编译或新的计时干扰。

### Task 8 — Independent review and Phase10 handback / 独立审查与交付

**Files:** Create `docs/superpowers/runs/2026-11-18-phase10-closeout.md`, execution/acceptance/integrity receipts and final master gap matrix; update the locator/entry docs only where scope and evidence justify it.

**Interfaces:** G1–G7 evidence and accepted manifests → explicit per-gate verdict, ordered review reports and a reviewable diff.

- [x] Obtain independent implementation SPEC then distinct-model QUALITY. The [review ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md#independent-implementation-review--独立实现审查) records both source verdicts and the signed-bundle publication regression for the QUALITY follow-up. / 独立实现审查按 SPEC→不同模型 QUALITY 顺序完成；打包跟进有实际签名包回归。
- [x] Close the approved native scope on the retained final candidate through the [gate table](../runs/2026-11-21-phase10-closeout.md); G7 uses the explicit amendment, while G6 physical topology remains user-deferred NOT TESTED. Source reviews do not replace native evidence. / 不把延期、源码审查或授权提示当作实测通过。
- [x] Verify linked evidence, current source/binary identities, served/default assets, terminal facts and exact original-reducer replay. Read retained test footers without adding historical counts as new tests. / 哈希、分位数和实际资产均已核验。
- [x] Report each master phase separately in the [handback](../runs/2026-11-21-phase10-closeout.md#5-master-roadmap-handback--原总方案逐期回看). Completion covers only this approved amended macOS slice, not native UI parity, Plugin Runtime, release readiness or Tauri removal. / 原总方案后续阶段未完成。
- [x] Record reasons, trade-offs, executed verification and skipped work in the closeout and preserve all evidence and existing branches/worktrees. No remote push/release/default switch is implied. / 保留原始失败记录与本机数据，未发布或切默认。
- [x] Self-review: original rejected verdicts remain intact; amended acceptance is separately machine-checked and independently reviewed, with deferred/untested scope explicitly retained. / 未把历史失败改成通过，也未用审查断言替代可复现核验。

## 7. Evidence continuity and pitfalls / 证据续接与已知陷阱

Private local corpus (preserve, do not bulk-add binaries to Git) / 本机私有证据目录：

```text
.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/
  isolated-matrix-40-20261118T185504/
    matrix.json
    measurement-source.json
    source-integrity.json
    restoration.json
    independent-integrity.json
    audit-program-execution.json
  final-audit-program/verify_current_matrix.py
  verify_final_handoff.py
  review-archive/
  final-review-note-reconciliation.md
  queue-projection-evidence.json
  queued-wire-20261118T145900/
  meter-fixes-20261118T155601/receipt.json
  quit-app-build-20261118T181620/receipt.json
```

The corpus is ignored/private and remains in the current checkout across a branch switch. It is **not automatically present in a fresh clone or another machine**. Committed acceptance/manifests summarize and hash it; a missing raw corpus is an evidence-availability issue, not permission to manufacture replacement receipts. Use new Phase10 outputs and target paths; do not mutate the immutable Phase9 auditor or old blocks.

Past mechanisms to retain / 需要记住的机制：

- Native capture on the external repository volume blocked `OpenOptions::open` before Tauri `builder.build`. Use owned local temporary capture, archive after exit. It was not a proved WebView construction hang.
- The shared native/collector monotonic clock and nonce/PID gate cover the launch boundary. Browser route/input/reconnect metrics use browser event timestamps, not IPC receive time; document time origin is captured once because WebKit getters can round differently.
- Input ends at the addressed `chat.send` response. New Chat may send a null session key; canonical response/run identity supplies the binding. Reconnect starts at the production reconnect-state commit and ends after replacement history application plus rendered frames.
- Bare LaunchServices coalition ID can represent one member; fork/join controlled proof exists. Host disappearance, tail-only members and stale known-absent WebKit records require the current strict validator; no broad name-only ownership or EPERM-as-absence shortcut.
- Public `proc_pidinfo` may return a full task record with zero RSS during exec. The accepted sampler preserves that actual value; PID identity admission and signal authority remain separate. Error/partial records still fail closed.
- All observed members must settle; `owned_drained` after `run_app` alone is insufficient. Unknown target WebKit in the final catalog cannot be dropped to obtain a PASS.
- Queue wire capture needs `stream.**`; production default busy input steers. The dedicated queue fixture disables existing `mid_turn_steering` only in its isolated root. The performance matrix retains the production default.
- The installed-service simulated execution transcript issue remains deferred. Inspect the known owners `src/gateway/execution_engine/simple.rs` and `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` only if that separate scope is authorized.

**Not done by this planning document / 本文未做:** Phase10 implementation or API feasibility proof, including the approved public-boundary implementation and delivery spike; OS permission changes, release/cross-platform/full-bundle acceptance, default client replacement, ADR/constitution changes, Plugin Runtime, Tauri removal or remote publication.
