# Panel Terminal Production-Host Acceptance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. User execution choice: restart in a new session; one main writer, sequential work. Use Pi-routed read-only independent review for the finished branch; do not dispatch writer subagents.

**Goal:** 补齐 Terminal 模块在真实 Panel host/SDK 上的确定性验收，修正过宽的证据声明，形成可复用的首个 Tab 迁移模板；真实 Gateway parity 单独判定。

**Architecture:** 保留已选的 Panel Shell + 动态 built-in Tab WASM 架构。浏览器必须从最终 staging HTML 启动真实 host WASM，由生产 Router、PanelShell、PanelLoader、PanelSdk 驱动 terminal module；只在 HTTP/WebSocket 服务端边界模拟 Gateway，不再用手写 HTML 导航或 SDK 替代生产路径。此设计需要维护少量 boot RPC fixture，成本高于独立 module smoke，但能验证原 fixture 绕过的主链路。

**Tech Stack:** Rust / Leptos CSR / wasm-bindgen / Bun / Playwright / existing JSON-RPC WebSocket protocol; no new frontend framework or runtime dependency.

**Spec:** `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`; bounded follow-up to `docs/superpowers/plans/2026-10-26-panel-terminal-tab-migration-plan.md` Task 6.

## Global Constraints

- 本计划基线：`main` 已快进合入 `terminal-live`，集成点 `bf157d61a`，包含 `fd6260168`；开始时再次检查实际 HEAD，不回退到旧 spike。实际默认分支是 `main`，不是不存在的 `origin/dev`。
- 实施从当前 main HEAD 创建独立 worktree/短分支 `terminal-host`；主工作区只用于当前文档规划与已授权的集成，不在主工作区实施代码。
- 只补首个 Terminal 模块的生产链验收与被测试证明的局部缺陷。不重做 Task 1–5，不迁移其他 Tab，不接社区安装链，不挂载 Capability/ZahirFacade，不改 `src/harness/` 或 PTY server business logic。
- 默认 legacy 发布构建、tracked `interfaces/webchat/dist/`、既有用户 Gateway/PTY/session 保持可回退。下一会话没有 merge/push/default flip 授权。
- UI teardown 只释放 UI/Owner/订阅/RAF/observer/polling/module handles，绝不因路由离开/disable 调用 `pty.close`。仅显式用户 close 或测试自己创建的 disposable PTY 可关闭。
- 每个 cargo 子进程（包括 build 脚本内部的 build/tree/版本调用）前，必须从当前 worktree 根执行 `python qa/browser_live/check_memory.py` 并确认 exit 0；cargo 从所属 package 目录运行，编译串行。
- git hook 也可能间接运行 cargo。提交前先检查 hook：不得无意违反 memory gate/执行目录规则；若 hook 仍与规则冲突，使用其明确允许的 bypass 并记录原因、实际 focused checks，不伪称 hook 通过。
- secrets 只通过进程内/受保护环境注入，不能进入仓库、报告、截图、trace、静态资产或日志。不要输出凭据环境变量值。
- 中文沟通、英文代码注释、文档中英双语；路径以当前 git root 派生，不在仓库文档写死机器绝对路径。

## Review Focus

1. 真实生产 host 被手写 fixture 偷换：必须看到真实 host WASM 启动、生产 root 和网络 RPC，不能只断言 module.mount 发生。（Task 2）
2. epoch 测试在旧条件下已经为真：使用本次 reconnect 前的基线计数、受控 pending response 和事件屏障，断言新 socket 生效、旧结果无法覆盖。（Tasks 1–3）
3. exit/resize/Canvas 只测“调用发生”：断言选中 session 的可见移除/回退、geometry 改变后的新 resize、目标 marker 实际画入 Canvas。（Tasks 1–3）
4. 版本错误或迟到加载损坏 Chat/Settings：真实路由往返、失败诊断/retry、双 root 清理必须可观察；不通过替身按钮声称成功。（Task 2）
5. 资产/内存数值被误称首屏 transfer 或回收：区分磁盘分发、浏览器实际请求、offline Brotli、JS/WASM timing 和不可测量项。（Task 4）

## Distance to the full design / 距完整设计的距离

以下按设计 Stage 定位，不使用未经定义的完成百分比。

| Design stage | 当前实际状态 / Current state | 未完成 / Remaining |
|---|---|---|
| 0 Contract & baseline | descriptor/lifecycle、部分构建体积已有 | 同条件真实加载/交互指标和内存边界仍不完整 |
| 1 Dynamic spike | 已选择 wasm-bindgen；真实 terminal artifact 可独立加载 | spike 的历史数据不等于生产 host 指标 |
| 2 Shell & registry | local registry/loader/SDK 已有；Shell 仍以单个 built-in descriptor 初始化 | 生产完整链验收；权威 extension projection、全局 lifecycle/UI 诊断尚未闭环 |
| 3 Built-in migration | Terminal 是首个独立模块；默认 legacy 保留 | Terminal host/live parity；其他 built-in Tab 逐个迁移；窄屏/phone/tablet 的一致契约 |
| 4 Community UI | trust/permission/descriptor 的基础契约已有 | manifest → install assets → descriptor projection → enable/update/uninstall 生产链；v1 trusted same-origin 模块；不把 WASM 当沙箱 |
| 5 Hardening & cleanup | 部分 loader 错误/cleanup 单测已有 | 版本/完整性/更新回滚、持续 leak/accessibility/multi-viewport 验证、移除旧 wiring、发布链与默认切换的独立验收 |

建议顺序：**本计划（Terminal host 验收）→ 通用 Shell/权威 projection 收口 → 按实测收益分批迁移其他 Tabs → 社区 UI lifecycle → 多端/加固/发布切换**。中后段有可并行部分，但每段需自己的边界与计划。本计划完成不表示整体 Panel 模块化完成。

## Known evidence gaps / 已确认的证据缺口

- `interfaces/webchat/tests/panel-terminal-protocol.test.mjs` 当前 `pageHtml()` 手写导航、`sdk`、connection；没有执行 staged `aleph_panel.js` / host WASM、生产 `PanelShell` 或 `PanelSdk`。
- epoch wait 使用 remount 前后的累计 subscribe/dispose 数，可能在 epoch 刚变化前已满足，不能证明这次 reconnect 恢复。
- exit 测试给不存在的 session 投递事件，只断言 emitter 自己记录 topic；Canvas 只比 data URL 变化；resize 可命中启动 resize；`pty.spawn` 有 handler 但没有断言创建/attach 行为。
- 未覆盖 delayed attach/subscribe、真正 disable/re-enable、missing/incompatible module 后真实 navigation/retry；报告对这些存在过宽表述。
- 上次只执行了 `opencode service status`，缺少该命令只证明 opencode 不可用，不能证明 Aleph Gateway 本身不存在；应保留历史命令并更正推论。
- 上次独立 Pi reviewer 超时没有结果；全仓 pre-commit 分别 DLL init failure/timeout，不能称 whole-branch independent review 或全仓 clippy 通过。

## Files and responsibilities / 文件落点

| Path | Responsibility / 职责 |
|---|---|
| `interfaces/webchat/tests/panel-terminal-protocol.test.mjs` | 保留 real module component fixture，改为确定性场景与可观察效果断言 |
| `interfaces/webchat/tests/fixtures/panel-gateway.mjs` (new) | loopback disposable HTTP/WS protocol fixture，原样服务 staged assets、模拟 boot/PTY RPC、控制 pending 响应/事件 |
| `interfaces/webchat/tests/panel-terminal-host.test.mjs` (new) | Playwright 完整 host fixture，从真实 staging HTML/WASM 运行生产导航和 SDK |
| `interfaces/webchat/package.json` | 添加独立 host fixture 命令，保留 component/build/loader 命令 |
| `interfaces/webchat/scripts/build-panel-tabs.mjs` | 内部每个 cargo 加 memory gate、修正 cargo 工作目录；必要时修正被 deep-link 用例证明的 public base 缺陷 |
| `interfaces/webchat/scripts/panel-tabs-report.mjs` | 核准分发 inventory，包括 font/其他实际资产；保留 offline estimate 标签 |
| `interfaces/webchat/src/{app.rs,panel/{shell,loader,sdk}.rs}` | 仅修复 full-host 测试揭示的实际问题；禁止为测试复制生产 Shell |
| `interfaces/webchat/tabs/terminal-ui/src/view.rs` | 仅修复证实的 stale list/attach/subscription/owner 问题，保持 Gateway session lifetime |
| `docs/reports/panel-terminal-tab-validation.md` | 保留历史并按证据层级修正；追加新命令/计数/live disposition |
| `docs/reference/{TERMINAL_RUNTIME,FEATURE_LOCATOR,SUBSYSTEM_ROUTING}.md` | 校准 contract vs verified behavior、真实代码/测试入口 |

## Task 1: Repair component-level evidence and build execution discipline

**Interfaces:** Consume existing terminal `describe()/mount(sidebar, content, sdk)` ABI and `invoke/subscribe/connection/locale` SDK contract. Produce deterministic component scenarios and a staged build runnable under the repository memory policy.

- [ ] **1. Establish baseline.** Read AGENTS, design, original plan/report, Terminal runtime/development/redlines/routing, this plan and restart prompt. Inspect `graphify-out/` through installed graphify instructions without unneeded rebuild. Check worktree/Git status. Locate applicable test skills; if absent record absence and use existing test conventions.
- [ ] **2. Create isolation.** `git worktree add .worktrees/terminal-host -b terminal-host HEAD` from verified clean main; if it already exists, inspect before reusing. Record exact source revision/tool/dependency resolution. Do not delete another session's changes.
- [ ] **3. Correct the build boundary.** In build script, run memory gate immediately before every cargo subprocess including `capture`; map host build/tree to `interfaces/webchat`, terminal build/tree to `interfaces/webchat/tabs/terminal`, preserving explicit target/output paths and `--locked`. Test gate rejection with an isolated stub process: cargo must not launch after a rejected gate. Do not add a second gate policy or silently treat exit 2 as allow.
- [ ] **4. Strengthen named component scenarios.** Add `selected_exit_changes_visible_tab`, `new_session_spawns_and_attaches_once`, `resize_changes_geometry_after_baseline`, `reconnect_creates_new_coverage`, `late_list_attach_subscribe_cannot_repopulate_disposed_roots`. Replace elapsed-time sleeps as synchronization with explicit deferred resolve/reject barriers and Playwright condition waits.
- [ ] **5. Pin effects.** Exit an existing selected session and remove it from fixture list; assert tab changes or disappears. Trigger real New action from UI and inspect exact spawn geometry/session identity then attach. Capture resize count/geometry first, resize container, require a subsequent changed geometry. Capture subscribe/dispose/attach baselines before epoch change and require new deltas for both exact topics and same selected PTY. Resolve old list/attach/subscribe only after disposal/new epoch and assert roots/current selection/screen cannot be overwritten.
- [ ] **6. Pin screen text.** Observe native Canvas text draws without suppressing them (test-only `fillText` observation forwarding to original) and retain screenshot evidence; require target Unicode marker after the relevant event. A changed data URL alone is insufficient. Inject a wrong topic and show that visible output does not change.
- [ ] **7. Run and fix only proved defects.** First demonstrate newly targeted assertions fail or that baseline coverage was absent; rebuild staged files after production source changes. `cd interfaces/webchat && node tests/panel-terminal-protocol.test.mjs` must report named scenario counts, zero failures; no real Gateway required. Commit `test(panel): strengthen terminal protocol assertions` after applicable focused checks.

## Task 2: Exercise the real production host through a wire fixture

**Interfaces:** New test helper `startPanelGateway({ assetRoot, publicBase })` returns `{ origin, gatewayUrl, controller, stop }`. `controller` exposes `calls()`, `holdNext(method)`, `release(token, response)`, `emit(topic, data)`, and `dropConnection()`; these are test-server controls, not product APIs. Bind only loopback, OS-assigned ports, independent test session IDs. Host runner uses Bun's existing HTTP/WebSocket support and installed Playwright; no new WS dependency.

- [ ] **1. Implement the wire fixture.** In `tests/fixtures/panel-gateway.mjs`, serve final staged HTML, loader, host JS/WASM, snippets/CSS and terminal files unchanged at `/` and `/panel/`, with SPA fallback and browser-expected WS endpoint. Follow actual `DashboardState` handshake/message types; do not invent a second protocol schema. Implement only observed boot methods with explicit responses; unknown RPC fails with named method, never a blanket success `{}`.
- [ ] **2. Start from production boot.** In `panel-terminal-host.test.mjs` import Playwright and helper; open final `index.html` and prove host WASM is requested/initialized before operating UI. Simulate valid connect in fixture server; use real UI navigation/DOM, not hand-written Chat/Settings buttons, `page.evaluate` calls to substitute navigation, replacement `DashboardState`, or SDK objects. Make locale deterministic through existing product support.
- [ ] **3. Prove lazy activation and preservation.** On Chat no terminal JS/WASM request. Navigate via actual Terminal control, mount both production roots, show marker, exercise input/spawn/resize. If current Shell requires a second selection click, record and test its actual behavior rather than hide it. Route Chat → Settings → Terminal: PTY identity preserved, no `pty.close`, no duplicated per-input delivery. Test normal and narrow non-phone viewport; phone remains separate scoped behavior, not an untested parity claim.
- [ ] **4. Prove production failure isolation.** For each root/prefixed base, inject missing module response and incompatible `api_major` descriptor. Assert failure diagnostic, Chat/Settings remain usable, restore valid response and use actual Retry/navigation path. Incompatible module must never mount or invoke PTY. Test direct SPA entry `/terminal` and `/panel/terminal` as well as root navigation; wrong asset base must fail the test rather than fall back to custom fixture HTML.
- [ ] **5. Prove new socket recovery.** Drop only fixture-owned WS connection; require new connect, exact-topic upstream coverage and re-attach of same selected PTY on new connection. Barrier-control pending old list/attach/subscribe; resolve after replacement and verify they cannot overwrite screen/selection or resurrect destroyed DOM. Assert input reaches current socket once.
- [ ] **6. Cover disable without inventing product scope.** Use existing production disable/reconcile API at Rust/browser ABI level with real loader if no user-facing enable action exists; do not add a fake disable button and call it host UI. Record which layer exercises disable/re-enable. Add an actual UI test later only when its lifecycle projection exists.
- [ ] **7. Run through `bun run panel:tabs:host:test`.** Add script `bun tests/panel-terminal-host.test.mjs`. Both public bases must execute positive cases, no skipped production-host cases; browser pageerror/console error/failed requests fail except explicitly injected failures. Always close helper/browser in `finally`. Commit `test(panel): exercise terminal through production host`.

## Task 3: Fix demonstrated production lifecycle gaps

**Interfaces:** Preserve current PanelLoader/PanelSdk/TerminalServices contracts. Do not change public protocol solely to simplify tests. Scope is conditional on failing tests from Tasks 1–2; do not implement speculative redesign.

- [ ] **1. Localize each failure.** Trace production `PanelShell`, `PanelLoader::activate/deactivate/reconcile`, `PanelSdk::invoke/subscribe`, and `TerminalContent` list/attach/polling to identify the owning boundary.
- [ ] **2. Add the regression first.** For stale async result verify both disposal and epoch/session identity at completion, not just request start. Use existing Rust lifecycle helpers/tests where available and the failing browser scenario that reached it.
- [ ] **3. Fix at the owner.** Invalidate old list/attach generation or capture/check epoch and owner only where actual race exists; always dispose late subscription handles. Avoid broad defensive rewrites/second registries. Route leave still cannot close PTY. Update runtime document distinguishing guaranteed contract from verified scenario.
- [ ] **4. Rebuild and rerun.** Re-run component and full-host failing cases plus affected package tests/check/clippy. Do not accept an old compiled WASM after editing source. Commit `fix(panel): fence stale terminal results` only if a production fix was needed; otherwise record Task 3 as no change needed with evidence.

## Task 4: Measure a reproducible candidate and independently check real Gateway prerequisites

**Interfaces:** Existing legacy/modular build recipes; report schema extended with explicit distribution inventory and browser observation fields, without renaming estimates to transfer. Live discovery is read-only; actual smoke uses an already authorized disposable Aleph target only.

- [ ] **1. Build on one source state.** Record revision plus tracked source diff/hash, profile, bindgen/opt, CSS/font inputs and Brotli quality. Build legacy with existing `just wasm` only after arranging memory gates/package cwd for its cargo child; otherwise invoke equivalent documented constituent build commands from package directories. Protect/restore baseline tracked dist without discarding user changes. Rebuild modular with corrected script; record exact manifest.entry and compiled descriptor equality.
- [ ] **2. Inventory all deployed assets.** Include fonts/binary/static files in total distribution or explicitly itemize excluded development metadata/precompressed duplicates. Host asset sum is not first-page transferred bytes. Use Playwright request/response observations during Chat and first Terminal activation for actual requested bytes/cache conditions; identify external resources separately. Use consistent per-file offline Brotli quality 11 for both arrangements.
- [ ] **3. Record timing honestly.** Browser Navigation/Resource Timing for host/module loads and an explicit initializer observation where supported; separate download vs initialize/instantiate proxy. If an exact metric cannot be observed, mark unavailable with reason. No invented RSS, GC reclamation, or whole-Panel lightweight claim. Maintain dependency-tree plus renderer-specific artifact-byte unlink proof, not symbol names only.
- [ ] **4. Discover Aleph correctly.** Read Aleph development/process docs and existing QA launch conventions; inspect command availability, service status, process names and listening endpoints without dumping process command lines/env/config containing credentials. Keep historical `opencode` failure but correct its implication. Missing opencode does not establish missing Aleph Gateway. Do not auto-start/install/restart/replace any real Gateway.
- [ ] **5. Conditional live smoke.** Only if an explicitly authorized disposable Aleph project/session and safely injectable credential exist: real Panel → production SDK → module → real Gateway WS, create unique disposable PTY, echo Unicode marker, paste/resize, route out/back preserving ID, drop only this test's connection and recover. Close only created PTY in `finally`, verify absence/closed status. If any prerequisite is missing, write BLOCKED reason and required next operator input; continue independent fixture/document work.
- [ ] **6. Make results distinguishable.** Output separate `component-fixture`, `production-host/wire-fixture`, `real-Gateway-live` verdicts/counts. Live missing target is BLOCKED, never PASS or silent skip. A full-host positive cannot be skipped just because live is blocked.

## Task 5: Evidence, review and handoff

- [ ] **1. Correct historical report in both languages.** Retain prior commands/measurements as dated history; narrow false lifecycle/error claims, explain component fixture bypassed production host. Preserve unavailable/blocked history and new verification separately; do not erase failures or assert Gateways were exhaustively absent.
- [ ] **2. Make ledger reproducible.** Commit exact commands, working directories, scenario counts, intentionally skipped items, tool versions, build revision/hash/entry, gate exit/available bytes, measured asset table, timing method, real live disposition and cleanup if run. Include previous review timeout and hook failures as history; do not repeat “all cargo gated” for a hook that was not.
- [ ] **3. Remove the existing glue skip where feasible.** After staging build: `ALEPH_PANEL_BRIDGE_GLUE=<absolute-generated-aleph_panel.js> node --test tests/panel_loader.test.mjs` from webchat. If generated bridge really cannot run under Node, document exact failure and retain positive browser generated-host evidence; never label a skip as exercised.
- [ ] **4. Run applicable checks once.** Each cargo gated from worktree root and executed in package dir: terminal native tests + WASM check/clippy, terminal-ui native tests/WASM check, Panel default and modular positive native tests + WASM check/focused clippy, `cargo fmt --all -- --check` from webchat. Node build/loader + both browser fixtures. Root `bun run check` only if script exists; otherwise unavailable. Docs link/entry checks and `git diff --check`; no need for unrelated full-core compilation by default.
- [ ] **5. Independent whole-branch read-only review via Pi.** Focus evidence honesty, owner/epoch races, permission boundary, PTY lifetime, staged asset integrity and reproducibility. Supply diff/source paths and bounded scope. A failed/timed-out child is not a passed review: account for it, fix findings and retry bounded review if meaningful; if still unavailable list that remaining limitation.
- [ ] **6. Update reference navigation.** `FEATURE_LOCATOR §6.11`, `TERMINAL_RUNTIME`, and stale terminal row in `SUBSYSTEM_ROUTING` must point to current shared renderer/module and both evidence levels, with live state accurate. No whole-project documentation rewrite.
- [ ] **7. Commit and stop.** Use `test(panel): verify terminal host protocol` if wire fixture passes/live blocked, or `test(panel): verify terminal live parity` only with real smoke/cleanup evidence. Ensure clean tracked worktree; report ignored validation assets explicitly and keep reusable build outputs until verification/review finishes. Do not delete junction targets, user caches or credentials. No merge/push/release default flip.

## Completion gates / 完成判据

| Gate | Required result |
|---|---|
| Evidence honesty | Report claims match executable assertions; previous gaps/failures retained |
| Component fixture | Named list/attach/subscribe/exit/spawn/resize/Canvas/teardown cases positive |
| Production host fixture | Final staged HTML + host WASM + production Shell/SDK + terminal WASM pass root/prefixed deployment and fault/reconnect cases |
| Lifetime | No UI teardown `pty.close`; stale responses fenced; one delivery per input |
| Build/metrics | Same-state legacy/modular inventory + real request accounting + double unlink; unavailable metrics explicit |
| Live | PASS with own-PTY cleanup, or explicit BLOCKED with precise prerequisite; cannot substitute fixture |
| Delivery | Docs accurate, applicable checks recorded, independent review result accounted, branch clean; no push/merge/default flip |

**This plan intentionally does not complete:** full registry projection from ExtensionManager; community installation/trust lifecycle; migration of remaining Tabs; mobile/tablet parity; final accessibility/leak matrix; default release flip. Those require subsequent bounded plans.

## Self-review / 计划自检

Spec subset coverage: Stage 2 production SDK/Shell proof and Stage 3 first Terminal acceptance map to Tasks 1–3; Stage 0 measurement gaps map to Task 4; limited Stage 5 lifecycle/failure coverage maps to Tasks 2/5. Stages 4 and remaining Stage 3/5 are explicitly deferred. Five review-focus risks each have effect-level tests. All new fixture interfaces are test-owned; product API remains unchanged unless a failing case proves a local bug. Execution is sequential native work in a fresh worktree as requested, after session restart.
