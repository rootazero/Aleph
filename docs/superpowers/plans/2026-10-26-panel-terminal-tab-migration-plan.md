# Panel Terminal Tab Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. 本会话仅编写计划；用户将在新会话启动实施。

**Goal:** 将真实终端 UI 从宿主 WASM 中拆成按需加载的独立 Tab，在可回退的模块化构建中保留现有导航、PTY 会话与终端行为。 / Move the real terminal UI into a lazy WASM Tab while preserving navigation and PTY behavior in an opt-in build.

**Architecture:** 沿用已验证的 `wasm-bindgen --target web`、DOM 双根节点与窄 SDK 边界。先把现有 Canvas 终端实现提取为无宿主依赖的共享 UI crate，再分别接入旧宿主适配器和独立 WASM 入口；模块化宿主不链接该共享 UI crate。本阶段只迁移 Terminal，其他入口继续使用原生 Leptos 视图。 / Reuse the proven loader; extract one terminal implementation behind local services, and select either the legacy host adapter or a separate WASM entrypoint at build time.

**Tech Stack:** Rust, Leptos 0.8 CSR, existing PTY protocol, wasm-bindgen, Bun build scripts, Playwright. 不引入新的终端模拟器、UI 框架或前端构建器。

**Spec:** `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`; completed baseline plan: `docs/superpowers/plans/2026-10-26-panel-tab-ui-capability-spike-plan.md`.

## Global Constraints / 全局约束

- 用户要求实现必须使用独立 Git worktree；保留 Subagent-driven 执行方式。每次仅一个子代理写当前 worktree，任务完成后做规格与代码审查。
- 当前主检出 `main` 为 `86ffeeb74`；上一阶段交付在本地 `panel-tab-spike`，HEAD 为 `c5146f30c64d19a70fce20c799b88e04fd0b5567`。它尚未合并或推送。**不能从当前 main 直接开始实现，也不能假定远端已有 spike。**
- 从本地已验收 spike 提交创建新分支 `panel-terminal-tab` 和新 worktree `.worktrees/panel-terminal-tab`。先核实以上提交和工作区；有分歧先比对，不 reset、不覆盖、不自动整合远端。
- 本机仓库没有 `origin/dev`；上一阶段已确认使用实际存在的 `main` 系列历史。这里明确基于本地 spike 延续开发，不虚构 `origin/dev`。不合并、不推送，除非用户另行授权。
- SDK 跨包只传 JS 对象、JSON、Promise、DOM Element、清理句柄；不跨 WASM 共享 Leptos Owner/Context/Signal、Rust trait object、`DashboardState` 或裸 WebSocket。
- 共享 Rust UI crate 是编译期复用，不是跨 WASM 共享内存；其依赖方向只能指向 Protocol 与 UI 基础库，不能依赖 `aleph-panel`、Core 或 Server。
- `aleph:terminal` 是真实终端身份；`aleph:terminal-pilot` 只留在上一阶段 fixture。保留 `aleph` 命名空间与现有 API major 校验。
- UI 卸载/Tab 禁用仅清理 UI 订阅、Owner、DOM、监听器、RAF/observer；不得调用 `pty.close`、停服务或清空 Core 会话。只有用户明确点击关闭终端会话才允许 `pty.close`。
- 非破坏性构建开关：默认旧构建保留；新增 `panel-modular` 供验收，不能本阶段直接切换默认发布。社区 manifest、安装/卸载管理、全体内置 Tab 开关和其他 Tab 拆包不在范围内。
- Phone 不新增终端功能。Tablet 现有终端能用，不能因宿主去链接而变成空白：模块化构建在原 Tablet 终端槽使用同一动态模块；不重做 Tablet/Phone Shell。
- 不改 `src/harness/`，不挂载 `ZahirFacade`，不实现 EffectClaim producer，不修改 PTY 服务器业务逻辑或借 UI 任务扩展 Capability runtime。
- 每次 cargo 调用前执行 `python qa/browser_live/check_memory.py`；exit 0 才继续。测试从对应 package 目录执行；重构前阅读适用测试规范。
- 编译串行；遵守 `.cargo/config.toml` 的 target-dir，以 `cargo metadata` 解析真实目录。长编译使用支持后台运行和持续反馈的工具，不反复重启编译。
- 文档中英双语，代码注释英文。保持 AGENTS.md/CLAUDE.md 入口不变；若确需修改必须同步两份。

## Review Focus / 审查重点

1. 首次打开 Chat 时终端 JS/WASM 未请求；模块化宿主依赖图和产物中也没有终端 renderer。测试归 Task 5/6。
2. PTY 列表失败、权限拒绝、重连和帧序列缺口不能变成误创建会话、空白假成功或越权。测试归 Task 1/2/6。
3. 快速离开、旧加载完成、卸载后异步返回、禁用后重新启用均不能写回已销毁 UI；原 PTY 仍存活。测试归 Task 3/4/6。
4. 非根路径部署、缺失/不兼容模块、Chat 与 Settings 子路由、Tablet 宽度变化仍可用。测试归 Task 4/5/6。
5. Canvas 字体、中文/组合键/粘贴、主题、终端多会话与 admin 错误文案不因拆包退化。测试归 Task 2/3/6。

---

## Phase Boundary and Trade-offs / 阶段边界与代价

上一阶段只证明了独立 pilot 的加载和清理，未迁移 `TerminalView`，未跑通真实 Shell/Gateway/WebSocket 全链路。不能把 pilot 的约 88.5 KB Brotli 当成真实终端包预算，也不能把未解耦宿主的体积当成轻量 Shell。

本计划选择共享 `terminal-ui` crate，而非复制一份终端页面：这样只保留一份 session/keymap/render 逻辑，但需要把 i18n、admin 错误映射、连接状态与事件收集改成显式本地服务。此为本计划提出的实现决策，不是用户此前逐项指定的 API。默认旧构建作为回退会暂时增加构建矩阵。其他大 Tab 尚未拆出，因此本阶段不承诺宿主整体体积大幅下降，更不承诺 OS 立即回收已实例化 WASM 内存。

本阶段不会重新造全量导航 registry：真实终端描述符进入现有 `TabRegistry`，其他内置菜单沿用现有单一模式表。导航层只把该终端入口映射到动态挂载，避免为尚未迁移的视图伪造 JS entry。

## Preflight / 新会话启动

- [ ] 阅读本计划、Spec、两级 AGENTS、`docs/reference/{SUBSYSTEM_ROUTING,DEVELOPMENT,TERMINAL_RUNTIME,REDLINES}.md`，以及原终端目录中的约束与 tests。启用 subagent-driven-development 与 using-git-worktrees。
- [ ] 在实际 git root 核实 `git status --short`、`git worktree list`、`git show --no-patch c5146f30c` 和 `git branch --list panel-tab-spike`。检查 `.worktrees/` 被忽略；已有目标 worktree 则先核对，不覆盖。
- [ ] 创建：`git worktree add .worktrees/panel-terminal-tab -b panel-terminal-tab c5146f30c64d19a70fce20c799b88e04fd0b5567`。若基线此后被用户更新，报告差异并保留本地 spike 修复。
- [ ] 本计划与配套 restart prompt 起初只写在主检出，可能尚未提交；把**这两份指定文档**复制到新 worktree 后纳入首个相关提交，不复制主工作区其余变化。已提交时可以从主检出读取这两份文件；不要为搬文档重放整段主分支历史。
- [ ] 记录 baseline：默认 Panel tests、feature registry/SDK/loader tests、JS loader tests、pilot artifact tests；不要声称上阶段六项 gated Shell E2E 已通过。运行前分别通过 memory gate。
- [ ] 建执行 ledger，逐任务记录提交、主验收、reviewer、实际命令及 skipped/blocked。子代理失败先读输出和活动记录；没有实质进展时同一问题最多三次有效尝试。

## File Responsibility Map / 文件责任

| Files | Responsibility |
|---|---|
| `interfaces/webchat/src/panel/{sdk.rs,loader/browser.rs}` | 只暴露窄 RPC、事件和连接快照；宿主管共享 transport |
| `interfaces/webchat/src/panel/{mod.rs,registry.rs,loader.rs,shell.rs}` | 复用现有身份、可见性、attempt fencing、加载与卸载；移除生产路径中的 pilot 专用假设 |
| `interfaces/webchat/tabs/terminal-ui/{Cargo.toml,src/lib.rs,src/host.rs,src/view.rs,src/keymap.rs,src/render.rs,src/session.rs,src/tabs.rs}` | 一份真实终端模型、Canvas renderer、双栏组件与本地服务契约 |
| `interfaces/webchat/src/platform/wide/views/terminal/mod.rs` | 默认旧构建的薄适配器，保留原 `TerminalView()` 调用入口 |
| `interfaces/webchat/tabs/terminal/{Cargo.toml,src/lib.rs,src/sdk.rs}` | 独立 `panel-tab-terminal` cdylib；JS SDK 适配及双根生命周期 |
| `interfaces/webchat/src/{app.rs,components/nav_menu.rs,platform/wide/views/mod.rs}` | 保留现有路由与非终端页面，终端按模式/feature 选择实现 |
| `interfaces/webchat/{Cargo.toml,index.html,package.json,styles/tailwind.css}` | opt-in feature、资产引导、脚本入口、独立 UI 的 CSS 扫描范围 |
| `interfaces/webchat/scripts/{build-panel-tabs.ts,panel-tabs-report.ts}` | 同源、同 build 的 manifest 和分包资产；不靠手填哈希路径 |
| `interfaces/webchat/tests/{panel-terminal.spec.ts,panel-terminal-live.spec.ts,panel-build.test.mjs}` | 可重复协议 fixture 与真实 Gateway 两条验证路径 |
| `justfile`, `scripts/check_panel_dist.mjs` | 可选择模块化构建、验证分包已进入最终发布目录 |
| `docs/reports/panel-terminal-tab-validation.md`, `docs/reference/TERMINAL_RUNTIME.md` | 结果、回退办法、限制与当前真实边界 |

Existing moved source files: `interfaces/webchat/src/platform/wide/views/terminal/{keymap,render,session,tabs}.rs`. Move their existing tests with the implementation; update old `crate::platform::wide::views::terminal::...` references to the new crate-local paths. 不保留第二份模型，不通过 `include!` 依赖旧目录。

## Local Interface Contract / 本地接口

下面是两个**同一 WASM 实例内部**使用的 Rust 接口；它们不能出现在跨包 ABI 中。`TerminalServices` 由旧宿主或独立模块分别构造。

```rust
pub struct TerminalConnection { pub connected: bool, pub epoch: u64 }
pub struct TerminalEvent { pub topic: String, pub data: serde_json::Value }
pub struct TerminalEventStream {
    pub drain: std::rc::Rc<dyn Fn() -> Result<Vec<TerminalEvent>, String>>,
    pub dispose: std::rc::Rc<dyn Fn()>,
}
pub struct TerminalServices {
    pub invoke: std::rc::Rc<dyn Fn(String, serde_json::Value)
        -> futures::future::LocalBoxFuture<'static, Result<serde_json::Value, String>>>,
    pub subscribe: std::rc::Rc<dyn Fn(String)
        -> futures::future::LocalBoxFuture<'static, Result<TerminalEventStream, String>>>,
    pub connection: leptos::prelude::Signal<TerminalConnection>,
    pub text: TerminalText,
}
pub struct TerminalText {
    pub new_tab: String,
    pub close_tab: String,
    pub read_refusal: String,
    pub write_refusal: String,
}
pub fn terminal_state(services: TerminalServices) -> TerminalState;
#[component]
pub fn TerminalSidebar(state: TerminalState) -> impl IntoView;
#[component]
pub fn TerminalContent(state: TerminalState) -> impl IntoView;
```

`TerminalConnection` 是 Copy/Clone/PartialEq，`TerminalState` 是可 Clone 的本地句柄；`TerminalEventStream` 的 dispose 必须幂等。服务闭包允许本地非 Send 值，使用 Leptos 的 local storage，不把 JS Function 或 Rc 强行声明 Send/Sync。`TerminalText` 的中英文字面值从当前 i18n/admin_refusal 复制必要键并对照测试，不能把整个宿主词典编入独立包。其余现有 UI 文案保持原样。

## Task 1: Complete the narrow PTY SDK boundary / 补齐终端所需 SDK

**Files:** modify `interfaces/webchat/src/panel/sdk.rs`, `interfaces/webchat/src/panel/loader/browser.rs`; tests alongside current SDK tests and `interfaces/webchat/tests/panel_loader.test.mjs` where browser glue is exercised.

**Interfaces:** retain `invoke`, `subscribe`, subscription `drain`/`dispose`; add `PanelConnectionSnapshot { connected: bool, epoch: u64 }` and `PanelSdk::connection() -> Result<PanelConnectionSnapshot, PanelSdkError>`, exported as `BrowserPanelSdk.connection() -> Result<JsValue, JsValue>` with JSON `{connected, epoch}`. No connection mutation or raw transport access.

- [ ] Add failing tests: `connection_snapshot_is_revoked_with_mount`; `pty_topic_is_ready_before_subscribe_returns`; `resubscribe_after_epoch_change_preserves_other_consumers`; `subscription_queue_is_bounded_and_disposed`. Assert revoke/dispose rejects connection access, exact-topic permissions remain mandatory, and one Tab cleanup does not unsubscribe another.
- [ ] Run existing SDK tests with `--features panel-tab-spike` to establish failures; unrecognized test filters or zero selected tests are failures of validation.
- [ ] Implement connection projection from existing `DashboardState.is_connected` and `connection_epoch`. For allowed PTY topics, establish existing host `subscribe_topic_ephemeral` coverage before resolving SDK subscribe. Re-establish on reconnect via the consumer's new epoch; never send global unsubscribe on UI dispose. Reject a late successful subscription after lease/mount revocation.
- [ ] Existing terminal traffic can reach ~60 frames/s; a paused drain must not accumulate indefinitely. Bound the SDK per-subscription queue to 256 events, evict oldest on overflow, and retain PTY sequence-gap recovery in Task 2. Document that this UI stream is lossy notification, not durable recovery. Do not implement generic replay, retry policies or backpressure middleware.
- [ ] Run from `interfaces/webchat`: `cargo test --lib --features panel-tab-spike panel::sdk` after the memory gate; run the targeted Node loader tests. Confirm positive selected test counts and existing permission/late-result cases remain green.
- [ ] Commit `feat(panel): expose terminal connection and topic lifecycle`; independent read-only review must accept the SDK boundary before consumers change.

## Task 2: Extract the real terminal implementation / 提取单一真实实现

**Files:** root `Cargo.toml`/`Cargo.lock`; create `interfaces/webchat/tabs/terminal-ui/` files listed above; modify old terminal wrapper, `interfaces/webchat/Cargo.toml`, and only needed i18n/admin error adapters.

**Interfaces:** implement the Local Interface Contract above. Preserve legacy `pub fn TerminalView() -> impl IntoView`. New crate name `aleph-terminal-ui`, library-only; no dependency on `aleph-panel`.

- [ ] Move existing session/keymap/render/tab-model tests into the new crate before migrating the renderer. Add regression assertions for list permission/transport failure causing **zero** `pty.spawn`, frame gap causing attach, reconnect restoring the selected session, and unmount with a pending attach leaving UI disposed without closing PTY.
- [ ] Run from new crate `cargo test --lib`; observe the service-facing tests fail before the port is implemented, and preserve every existing pure terminal test.
- [ ] Move the current Canvas view, keeping server VT emulation, frame buffering/sequence rules, title/exit tab reconciliation, resize, keyboard and bracketed-paste behavior. Replace `DashboardState` calls with `TerminalServices`. Replace the accidental dependency on `crate::views::voice::wav::base64_encode` with a direct, focused encoding implementation using an already available lightweight dependency; verify raw control and non-ASCII bytes. No voice module import.
- [ ] Keep one `TerminalState` shared by sidebar and content: sidebar lists/selects sessions and creates a session; content retains Canvas and current terminal tab interactions. Both use the same `TabModel`; no parallel selection state. Old wrapper may render content in its current layout without introducing the new sidebar.
- [ ] Collect exact-topic events through `TerminalEventStream` and drain while mounted. One repaint/collection loop per terminal instance; dispose streams on unmount and before reconnect replacements. Preserve `try_` reads and generation/session guards after every await. A frame gap after bounded-queue loss uses the existing attach recovery, not a synthetic success.
- [ ] Make `aleph-terminal-ui` an optional `aleph-panel` dependency behind default feature `legacy-terminal`; the old wrapper builds services from host state and current translations. Implement default build compatibility first; no source copy remains in old files.
- [ ] Run new crate tests, default Panel tests, native/WASM check and focused clippy after separate memory gates. Verify dependency direction with `cargo tree -p aleph-terminal-ui --target wasm32-unknown-unknown`; inspect only its direct dependency subtree, not all registry output.
- [ ] Commit `refactor(panel): extract reusable terminal ui`; review especially disposed-owner handling, privilege errors, font policy and original PTY test preservation.

## Task 3: Build the production terminal WASM entry / 构建真实独立入口

**Files:** create `interfaces/webchat/tabs/terminal/{Cargo.toml,src/lib.rs,src/sdk.rs}`; workspace Cargo files; independent module unit/browser tests.

**Interfaces:** package `panel-tab-terminal`, `crate-type = ["cdylib", "rlib"]`; ABI follows the existing pilot: `describe() -> JsValue`, `mount(sidebar_root: web_sys::Element, content_root: web_sys::Element, sdk: JsValue) -> Result<MountHandle, JsValue>`, `MountHandle::unmount(&self)`. Descriptor key `aleph:terminal`, API version matches existing `PANEL_UI_API_VERSION`. The build's manifest entry and the module's descriptor must match. 编译输入 `PANEL_TERMINAL_ENTRY` 是该 entry 的唯一构建源；非发布的独立测试允许默认 `./tabs/terminal/dev/terminal.js`，正式打包脚本必须显式注入。Task 5 同时为 manifest 与模块编译提供正式值。

- [ ] Add failing tests asserting exact descriptor/permissions, two-root mount, empty roots after two unmount calls, and cleanup after partial mount failure. Inject a late list/attach response and assert no disposed-owner error or Core close call.
- [ ] Implement only a JS SDK-to-`TerminalServices` adapter. Mirror SDK connection snapshots into a **module-local** signal; own and cancel its update/collection loop. Convert subscriptions to `TerminalEventStream`; call dispose on all returned handles, including handles returned after module teardown.
- [ ] Both render roots share one module-owned state/Owner. Mount rendering under that Owner and release all mount handles/Owner on unmount; do not rely solely on replacing innerHTML. Draw the real Canvas renderer, not a pilot control or a copied visual substitute.
- [ ] Exact RPC grants: `pty.list`, `pty.spawn`, `pty.attach`, `pty.input`, `pty.resize`, `pty.close`, `config.get`. Exact subscription grants: `events.subscribe:pty.screen`, `events.subscribe:pty.exit`. Retain server admin/policy enforcement; `config.get` is a method-level grant in current SDK, not a claim of parameter-scoped sandboxing.
- [ ] Read locale and theme through existing SDK/CSS semantics; preserve effective server font policy and native error messages. No second WebSocket, no token injection into module assets, no raw Core import. Same-origin trusted code remains a trust boundary, not security isolation.
- [ ] Run new crate unit tests, WASM check/clippy, wasm-bindgen generation and a real generated-glue mount test. Assert `cargo tree -p panel-tab-terminal --target wasm32-unknown-unknown` contains no `aleph-panel` or `alephcore`.
- [ ] Commit `feat(panel): add standalone terminal wasm tab`; review resource ownership and all JS/Rust closure cleanup.

## Task 4: Integrate with existing navigation / 接入真实导航与回退

**Files:** modify `interfaces/webchat/src/panel/{mod.rs,shell.rs,registry.rs,loader.rs}` only where needed, `interfaces/webchat/src/app.rs`, `interfaces/webchat/src/components/nav_menu.rs`, `interfaces/webchat/src/platform/wide/views/mod.rs`, `interfaces/webchat/Cargo.toml`; route/loader tests.

**Interfaces:** add feature `panel-modular = []`; retain `panel-tab-spike = ["panel-modular"]` for fixture compatibility and update cfg use consistently. Production candidate build uses `--no-default-features --features panel-modular`; default enables `legacy-terminal`. Compile-error explicitly rejects a modular build with `legacy-terminal` still enabled. Dynamic terminal uses the existing TabRegistry/PanelHostState and TabLoader, not a second lifecycle registry.

- [ ] Write failing route tests: `/chat`, `/settings/providers` and current other modes still resolve to existing views; `/terminal` selects `aleph:terminal` once; back/forward restores it; direct terminal URL loads it; missing module does not block Chat/Settings. Test default and modular feature combinations.
- [ ] Replace the spike's whole-wide-screen takeover: module Shell is the terminal route's dual-pane container, while current nav and existing layouts remain available. Use current router location as authority; registry selection and browser history must agree. Do not keep two independent active-Tab states.
- [ ] Retain existing `NavMenu` single mode list for unmigrated built-ins. Its terminal entry targets the registered dynamic module in modular builds; loading errors show retry plus navigation to other entries. Do not register fake JS files for legacy modes.
- [ ] Conditionally compile the old terminal import/module only under `legacy-terminal`. In modular builds, both Wide and existing Tablet terminal slots mount the dynamic module; Phone retains its current unsupported behavior. Width changes destroy the prior mount before reusing roots; other Tablet/Phone routes keep their current implementations.
- [ ] Render terminal only while its route is active. The existing hidden wrapper must not eagerly mount terminal on Chat merely because it uses `display:none`. Test zero terminal mounts and requests before first activation.
- [ ] Exercise existing disable/re-enable registry transitions through tests: revoke current UI, empty roots, keep PTY, recover on re-enable. Do not add a new persisted settings toggle or claim backend-provider shutdown is implemented.
- [ ] Run default Panel tests and modular tests with positive counts; run loader Node tests. Assert `cargo tree -p aleph-panel --no-default-features --features panel-modular --target wasm32-unknown-unknown` excludes `aleph-terminal-ui`; inspect source imports as well as dependency output.
- [ ] Commit `feat(panel): integrate dynamic terminal navigation`; review route authority, hidden eager mounts, Tablet compatibility, and missing module recovery.

## Task 5: Package modules with the real build / 接通最终资产链

**Files:** create `interfaces/webchat/scripts/build-panel-tabs.ts`, `panel-tabs-report.ts`, `interfaces/webchat/tests/panel-build.test.mjs`; modify `interfaces/webchat/{package.json,index.html,styles/tailwind.css}`, `justfile`, `scripts/check_panel_dist.mjs`; reuse spike loader/report utilities when practical.

**Interfaces:** `bun run panel:tabs:build` builds the modular host + real terminal module; `bun run panel:tabs:report` emits same-profile asset sizes. New `just wasm-modular` stages the result to the same actual Panel distribution location used by existing `just wasm`. Leave `just wasm` default behavior unchanged.

- [ ] Add failing asset tests for root and `/panel/` public bases, missing terminal WASM, stale/mismatched descriptor identity, and traversal outside the staged root. Assert manifest URLs resolve to existing final-release files, not only `.panel-spike` files.
- [ ] Build the host with `--no-default-features --features panel-modular` and terminal crate separately using existing `wasm-release` profile and matching wasm-bindgen CLI. Apply the repository's existing WASM optimization/feature validation discipline to both; do not silently substitute the spike's permissive validator.
- [ ] Generate a same-build JSON manifest containing the trusted built-in descriptor and actual relative JS entry. Use `./tabs/terminal/<build-id>/terminal.js`, with `<build-id>` derived **before compilation** from source/build inputs; pass that exact entry as `PANEL_TERMINAL_ENTRY` to the terminal crate and manifest writer. Do not derive the directory from final JS bytes that themselves encode the entry. Stage WASM and snippets beside that JS file. Serve URLs relative to the explicit deployment base; keep the corrected public-root loader bridge, avoid a second wasm-bindgen snippet loader.
- [ ] Make final distribution staging include loader, manifest, module assets, CSS and fonts. This repo uses Tailwind CSS `@source`, not a `tailwind.config.js`: add `@source "../tabs/terminal-ui/src/**/*.rs";` and `@source "../tabs/terminal/src/**/*.rs";` in `styles/tailwind.css` so extracted classes survive the production scan. Do not bundle another CSS reset.
- [ ] Boot consumes this bundled built-in projection; it is not an ExtensionManager/Capability runtime authority. Missing manifest/module produces an actionable terminal error while old pages remain usable. Keep pilot assets out of normal candidate distribution.
- [ ] Run `node --test tests/panel-build.test.mjs` from webchat and `bun run panel:tabs:build`; run the actual distribution checker. Validate root and prefixed deployment using final staged output.
- [ ] Commit `build(panel): package lazy terminal module`; review final-file presence, API/manifest agreement and cache consistency.

## Task 6: Prove parity and record measurements / 真实链路与测量

**Files:** create `interfaces/webchat/tests/panel-terminal.spec.ts`, `panel-terminal-live.spec.ts`, `docs/reports/panel-terminal-tab-validation.md`; modify package test scripts and `docs/reference/TERMINAL_RUNTIME.md`; add fixture helpers beside tests only when reused.

**Interfaces:** `bun run panel:tabs:test` runs deterministic browser tests against final staged candidate assets; `bun run panel:tabs:test:live` explicitly requires a supplied live Gateway URL and authorized credential through environment/server injection. Reuse existing auth and WS proxy conventions; never write secrets into docs or generated static assets.

- [ ] Implement a disposable Gateway protocol fixture with valid typed PTY frames. Test actual Shell + SDK + JS glue + terminal module, with frame rendering, input bytes, resize, spawn/list failure and epoch reconnect. Assert visible screen/content and PTY actions, not just that `mount()` was invoked. Clearly label this protocol-fixture evidence.
- [ ] Browser matrix: (a) Chat startup: no terminal requests/mount; (b) first activation: one module load, sidebar+Canvas usable; (c) switch away and back: cached import, new clean UI, same PTY; (d) permission/policy/list failure: correct error, no unintended spawn; (e) disconnect/reconnect: topics ready, same selected PTY attach; (f) missing/incompatible entry: retry and other pages work; (g) quick switch/dispose/disable and width change: no late DOM mutation, repeated listeners or accidental PTY close; (h) root and `/panel/` deployment.
- [ ] Run an explicit live Gateway smoke test: create a disposable terminal session, execute an echo marker, observe it in the real Canvas/client screen, send Unicode/bracketed paste and resize, switch to Chat then re-enter, drop/recover transport and verify the same session. Only close this test-created session as explicit test cleanup. Do not disturb existing user sessions or installed server processes. If no authorized disposable Gateway is available, record BLOCKED and stop short of production acceptance; do not relabel fixture tests as live.
- [ ] Measure old default and modular candidate using the same source revision, `wasm-release`, optimizer, CSS and Brotli settings. Report host-only bytes, terminal bytes, total distribution bytes, and first-page transferred bytes separately. Record load/instantiate durations and WASM linear-memory capacity with their limits; no fabricated percentage target or RSS/reclamation claim.
- [ ] Provide two independent proofs of code removal: modular host dependency graph excludes terminal-ui; artifact inspection shows terminal-specific module code/data moved to terminal artifact (account for stripped names by comparing an inspection build). A smaller total alone is insufficient. Other Tabs still in host must be stated.
- [ ] Run relevant package tests for shared protocol (if changed), terminal-ui, terminal module, default and modular Panel; native/WASM check, focused clippy, `cargo fmt --all -- --check`, Node loader/build tests, browser fixture and live suites. Gate each cargo command. Root `package.json` currently has no `check` script: record `bun run check` as unavailable under generic global guidance and use the repository's Rust validation commands; do not invent a success or create an unrelated script.
- [ ] Write a bilingual report with exact commits/tool versions/commands/counts, measured files and settings, skipped/blocked reasons, legacy rollback command, and remaining stages. Update TERMINAL_RUNTIME to explain UI-module teardown vs backend PTY lifetime. Keep tracked evidence in docs, not only ignored `.superpowers` logs.
- [ ] Run independent whole-branch review in Pi, compare against the original Spec and this plan, resolve blockers, then commit `test(panel): verify dynamic terminal parity`. Report branch/worktree and clean status. No default-release flip, merge or push.

## Verification Commands / 验证命令约定

每条 cargo 命令执行前从仓库根运行 `python qa/browser_live/check_memory.py` 并检查 exit 0，再切到表中目录执行；不能把 memory gate 的失败忽略后继续。 / Run the memory gate before each cargo command; execute tests from the owning package.

| Working directory | Command | Required evidence |
|---|---|---|
| `interfaces/webchat` | `cargo test --lib` | 默认旧构建的完整测试通过，记录计数 |
| `interfaces/webchat` | `cargo test --lib --no-default-features --features panel-modular panel::` | SDK/registry/loader 实际选择到正数用例；用于 Task 4 之后 |
| `interfaces/webchat` | `cargo check --lib --target wasm32-unknown-unknown --no-default-features --features panel-modular` | 模块化宿主 WASM 编译通过 |
| `interfaces/webchat` | `cargo clippy --lib --no-default-features --features panel-modular -- -D warnings` | 模块化宿主无新增警告 |
| `interfaces/webchat/tabs/terminal-ui` | `cargo test --lib` | 迁移后的全部模型/render/keymap 测试与新增服务用例 |
| `interfaces/webchat/tabs/terminal` | `cargo test --lib` | 独立入口 descriptor/lifecycle 用例 |
| `interfaces/webchat/tabs/terminal` | `cargo check --lib --target wasm32-unknown-unknown` | 独立真实终端 WASM 编译通过 |
| `interfaces/webchat/tabs/terminal` | `cargo clippy --lib --target wasm32-unknown-unknown -- -D warnings` | 独立 WASM 无新增警告 |
| `interfaces/webchat` | `node --test tests/panel_loader.test.mjs tests/panel-build.test.mjs` | 真正执行 loader/build 用例，必要 glue 环境未设置不能冒充已测 |
| `interfaces/webchat` | `bun run panel:tabs:build` | 最终模块化发布资产完整、校验通过 |
| `interfaces/webchat` | `bun run panel:tabs:test` | 完整 Shell/SDK/模块协议 fixture 用例全部执行 |
| `interfaces/webchat` | `bun run panel:tabs:test:live` | 显式真实 Gateway 前置条件满足且 live 用例通过；否则 BLOCKED |
| repository root | `cargo fmt --all -- --check` | 正确 worktree 内的格式检查通过 |

Task 1 在 feature 重命名前沿用 `cargo test --lib --features panel-tab-spike panel::sdk`；Task 4 之后使用表中的 `--no-default-features --features panel-modular`，不要把不兼容的 legacy/default feature 混入模块化验证。若测试或构建会写 VERSION、生成资产或 lockfile，检查 diff，避免把测试副作用混入产品提交。

## Completion Criteria / 完成判据

- [ ] Real terminal code has one implementation; modular host cannot compile/link it; standalone module uses only SDK and local UI dependencies.
- [ ] Existing non-terminal pages remain accessible; terminal has dual roots and preserved PTY behaviors; cached re-entry does not duplicate mounts or kill sessions.
- [ ] Final distribution includes usable assets and works under a deployment prefix; production CSS includes the extracted view's classes.
- [ ] Deterministic full-chain browser tests pass, and live Gateway parity is either actually verified or explicitly blocks production acceptance.
- [ ] Measured host/terminal/total/first-load sizes and evidence are committed; default legacy rollback remains available.

## Deferred / 未做

社区 UI 插件安装、签名/信任设置、后端 descriptor projection、Capability provider 的真实启动/停机、所有内置 Tab 的注册开关、Memory/Teams/Agents/Settings 拆包、Phone 新终端界面、完整 Tablet Shell 模块化、默认发布切换，以及 OS 内存立即回收保证。 / Community installation, backend provider lifecycle, other Tab migrations, mobile redesign, rollout and OS memory reclamation guarantees remain outside this phase.
