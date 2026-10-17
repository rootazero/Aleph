# Aleph 右侧栏优化（阶段 1–6）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 `docs/research/2026-10-15-rightside-optimization-plan.md` 实现右侧栏 slot 化、flex 第三列和阶段 3–6 的只读 slot，保留现有 deep-link、持久化、`_blank` fallback 与已有 browser transport 行为。

**Architecture:** `RightPaneSlot` / `RightPaneRegistry` 是唯一 slot 描述与注册源；`WorkspaceState` 只持有运行时开关、选中 slot、宽度和持久化状态。阶段 2 把 host mount 从 chat view 提升到 app shell，并提供 flex/legacy 双 path。阶段 3–6 只把既有 subagent、capability projection、attachment/artifact、browser transport 接到 slot，不在 Core 增加 UI 业务逻辑或复制事实源。

**Tech Stack:** Rust / Leptos WASM / tokio / 现有 JSON-RPC 与 browser transport；不新增重依赖，不改 TUI 阶段 7，不改 capability producer、ACP inbound server、Safe Replay 或 `BrowserWorkspace` 核心模型。

**Source specs:**

- `docs/research/2026-10-15-rightpane-stage1-slot-spec.md`
- `docs/research/2026-10-15-rightpane-stage2-host-switching-spec.md`
- `docs/research/2026-10-15-rightpane-stage3-subagent-slot-spec.md`
- `docs/research/2026-10-15-rightpane-stage4-capabilities-slot-spec.md`
- `docs/research/2026-10-15-rightpane-stage5-attachments-slot-spec.md`
- `docs/research/2026-10-15-rightpane-stage6-controlled-browser-slot-spec.md`

## Global constraints

- 先完成阶段 1 的 trait/registry 契约，再实现 host 迁移和具名 slot。
- registry 构造后只读；slot id 是持久化键；`WorkspaceState` 不反向写 registry。
- 重复 slot id 必须返回 `RegistryError::DuplicateSlotId(id)`；启动注册失败使用 `expect("right pane registry")`，测试可捕获断言。
- `WorkspaceState::default_open` 由 registry 注入；默认打开 `rps.subagent_tree` 与 `rps.artifacts`，但 subagent 与 runtime agents 保持两个事实源/两个 slot。
- 阶段 2 的宽度契约为将请求宽度限制在 `[280, max(280, viewport * 80%)]`，小 viewport 仍返回 280；保留 `aleph.panel.workspace_w` 与 `aleph.panel.layout_mode` 持久化。
- flex path 默认启用；`localStorage["aleph.right_pane.flex"] == "0"` 才走 legacy path；`?legacy_pane=1` 仍可覆盖进入 legacy。
- 阶段 4 按详细 spec §4（原文“本阶段不实施代码，只列方向”）保留 capabilities 骨架与只读接入方向。WASM 当前没有 Public descriptor/schema/attach wire，既有 operator/loopback diagnostics status 不能代替该契约；不增加新桥或 runtime mount。
- 阶段 5 草稿仍最多 10 个、单文件 10 MiB；Tray 只保存显式 pin/upload，默认显示最近访问 7 天内最多 50 条，超出只折叠和提示，不自动删除，单条上限 50 MiB。按 spec 后文风险修正，IndexedDB 保存全部真实字节，localStorage 只保存 metadata；事务完成后才发布引用。缩略图仅 `image/*` 且 `<2 MiB`。
- 阶段 6 复用 standalone browser transport；现有 viewer 已接收显式 session/origin props，route 才读取 location。保留显式新授权的 `Open in tab` 与 `_blank`，已消费的 grant 不重用；不引入新 iframe/webview，不写回 `BrowserWorkspace`。
- 复杂 UI 留在 `interfaces/webchat`；Core 只提供已有协议/投影数据。slot unmount 必须依赖 Leptos 生命周期取消订阅，不缓存已订阅 slot id。
- 每阶段只修改该 spec 的文件边界；发现设计冲突时暂停该阶段并记录精确证据，不通过复制事实源绕过冲突。

## Review focus

1. slot registry 是否是唯一注册/排序/默认打开来源，重复 id 是否 fail-closed。
2. chat view 是否彻底移除 workspace host，app shell 是否成为唯一 flex host，legacy path 是否仍可回归。
3. subagent tree、runtime agents、capabilities、attachments 和 browser 是否各自复用已有事实源，没有重复订阅或后台状态缓存。
4. 所有异步流是否在卸载时取消；capability snapshot 与 diagnostic 是否只读；browser 失败是否能回退 `_blank`。
5. 阶段验收中的静态扫描、纯函数测试、宽度边界测试和 Webchat 测试是否真实运行并通过。

## File structure and stage tasks

### Task 1 — 建立 RightPaneSlot 与 registry 契约

**Files:**

- Create: `interfaces/webchat/src/components/right_pane/mod.rs`
- Create: `interfaces/webchat/src/components/right_pane/slot.rs`
- Create: `interfaces/webchat/src/components/right_pane/registry.rs`
- Create: `interfaces/webchat/src/components/right_pane/slots/mod.rs`
- Modify: `interfaces/webchat/src/components/mod.rs`
- Modify: `interfaces/webchat/src/state/layout.rs` only for registry-derived default-open/selected-slot state if required by the spec
- Tests: the new registry/slot module tests

**Interfaces:**

```rust
pub trait RightPaneSlot {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn icon(&self) -> Option<&'static str>;
    fn order(&self) -> u16;
    fn visible(&self, ctx: &SlotCtx) -> bool;
    fn default_open(&self) -> bool;
    fn render(&self, ctx: SlotRenderCtx) -> AnyView;
}

pub struct RightPaneRegistry { /* immutable after construction */ }
impl RightPaneRegistry {
    pub fn register(&mut self, slot: Arc<dyn RightPaneSlot>) -> Result<(), RegistryError>;
    pub fn get(&self, id: &str) -> Option<Arc<dyn RightPaneSlot>>;
    pub fn iter_visible(&self, ctx: &SlotCtx) -> impl Iterator<Item = Arc<dyn RightPaneSlot>>;
    pub fn default_open(&self) -> BTreeSet<&'static str>;
}
```

- [x] Add the trait, render context and closed `RegistryError` with duplicate id.
- [x] Make registration reject duplicate ids without replacement; sort iteration by `order`, then stable id.
- [x] Add registry tests for duplicate rejection, order, visibility, default-open extraction and immutable lookup behavior.
- [x] Register placeholders or concrete stage-1 slots exactly as the spec requires, without moving stage-3+ logic early.
- [x] Verify the registry does not own `WorkspaceState`, persisted state, subscription caches or UI layout state.

**Acceptance:** `RegistryError::DuplicateSlotId(id)` is catchable; default-open ids come from slots; registry tests pass; no second slot list is introduced.

### Task 2 — Move WorkspacePanel to the flex host

**Files:**

- Modify: `interfaces/webchat/src/app.rs`
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs`
- Modify: `interfaces/webchat/src/state/layout.rs`
- Modify: the relevant `tailwind.css`/webchat style token file
- Tests: layout/state and static contract tests

- [x] Mount the right pane in `app.rs` as the third flex child and remove the chat-view `<WorkspacePanel>` mount.
- [x] Remove the absolute-position/right-offset width implementation from the flex path; preserve the pane’s content, drag handle, collapse and keyboard behavior.
- [x] Keep `--aleph-workspace-w` as the shared width token and preserve `aleph.panel.workspace_w` and `aleph.panel.layout_mode` read/write behavior.
- [x] Preserve `LayoutMode::ChatOnly` (aside hidden) and `LayoutMode::Split` (aside shown), including restoring the prior width.
- [x] Implement `clamp_width(px, viewport_w)` by clamping the requested width into `[280, max(280, viewport_w * 80%)]`, including the specified low-width and zero viewport behavior.
- [x] Implement `flex_path_enabled()` with default `true`, localStorage value `"0"` as false, and the `?legacy_pane=1` override. Legacy path must remain available for rollback/visual comparison.
- [x] Preserve the known resize limitation documented by the spec and add the required width transition.
- [x] Add/adjust the ten stage-2 static and parameterized checks, especially no `<WorkspacePanel>` in `chat/view.rs`, at least one right-pane mount in `app.rs`, no absolute aside in flex code, no `pr-[var(--aleph-workspace-w)]`, and no right-side calc chrome.

**Acceptance:** stage-2 unit/static checks pass; flex is the default; legacy path still works; width/layout persistence is unchanged.

### Task 3 — Add subagent and artifact slots

**Files:**

- Create/modify: `interfaces/webchat/src/components/right_pane/slots/subagent_tree.rs`
- Create/modify: `interfaces/webchat/src/components/right_pane/slots/runtime_agents.rs`
- Create/modify: `interfaces/webchat/src/components/right_pane/slots/artifacts.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/subagent_tree/state.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/subagent_tree/visuals.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/subagents.rs` or the actual `/dashboard/subagents` route file
- Modify: `interfaces/webchat/src/components/right_pane/mod.rs` and registry wiring

- [x] Move the existing recursive subagent-tree rendering into `rps.subagent_tree`, default open/order 100, using one initial `subagent.tree` RPC plus `TREE_TOPIC` deltas.
- [x] Keep `/dashboard/subagents` as a full-screen deep-link; opening it expands the right pane and selects `rps.subagent_tree` when needed.
- [x] Add a separate `rps.runtime_agents` slot for `runtime.agents.list`; do not merge it with background subagent tree data. Keep the sidebar live-agent row semantics.
- [x] Keep in-chat `SubagentNode` collapsed to its existing one-line form and expose an explicit “Open full view” action where the spec requires it.
- [x] Add the inline detail drawer for selected subagent transcript/metadata; subscriptions must terminate with component unmount.
- [x] Make default open include subagent tree and artifacts, while slot tab switching controls visibility of the active body.

**Acceptance:** subagent tree renders in the right column, runtime agents remain separate, deep-link expands/selects correctly, and no slot stores an “already subscribed” id cache.

### Task 4 — 确认 capabilities 只读方向与接入边界（spec-only）

- [x] 保留 `rps.capabilities` 的 registry 骨架。
- [x] 核对 Core 的 `current_snapshot()`、`attach(Scope::Public)` 和独立 `diagnostic_status()` 与实际 Panel wire 边界。
- [x] 记录现有 operator + loopback + flag 限定的只读 diagnostics/status 工具不能提供完整 Public descriptor/schema/attach；不以 `tools.catalog` 拼造原子 capability snapshot。
- [x] 遵循阶段 4 spec §4 的“不实施代码，只列方向”，不引入 producer、EffectClaim 写入、runtime mount 或新 RPC。

**Acceptance:** 边界核对见 `.superpowers/sdd/2026-10-16-rightpane-optimization/stage4-interface-check.md`；这是方向与可行性核对完成，不宣称完整 capabilities UI 已完成。

### Task 5 — Upgrade attachment chips and add the pinned tray

**Files:**

- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs`
- Create/modify: `interfaces/webchat/src/components/right_pane/slots/attachments.rs` or the spec-defined tray file
- Modify: `interfaces/webchat/src/components/right_pane/slots/artifacts.rs`
- Modify: `interfaces/webchat/src/components/right_pane/mod.rs` and registry wiring
- Tests: attachment pure functions, tray serialization/LRU, thumbnail decisions

- [x] Add `mime_to_icon(mime: &str) -> IconKind` with the eight spec mime families and fallback.
- [x] Add image thumbnail generation for `image/*` and files `< 2 MB`; key thumbnail state with the stable attachment key and release data URLs when the draft vector resets.
- [x] Upgrade draft chips to the approved A2 icon/thumbnail form, preserving 10 files and 10 MiB/file. Quota/error chip UI remains v2 per the approved decision.
- [x] Add explicit draft/artifact pins and Tray uploads; fetch actual artifact bytes and await IndexedDB transaction completion before publishing metadata. All bytes use IndexedDB, with metadata only under localStorage `aleph.tray.v1`, per the later spec risk correction.
- [x] Default to the 50 most recently accessed items within 7 days, with count/reveal for folded rows and no automatic deletion; enforce the actual 50 MiB byte limit.
- [x] Keep draft send/reset payload and lifetime unchanged; no draft LRU.
- [x] Verify icons, strict thumbnail predicate, stable middle deletion, serialization, folding boundaries, concurrent publication, durable reload/reuse/download/remove and failure behavior.

**Acceptance:** draft limits remain intact; explicit pins survive reload as identical bytes; folding preserves all source data; no second artifact authority. Source `c44153862` and follow-up corrections received independent review; compiled Stage 5 QA passed 15/15 grouped cases on `b46653a7a`. The final release reruns this against the latest owner-retention build.

### Task 6 — Add the controlled browser slot with fallback

**Files:**

- Create: `interfaces/webchat/src/components/right_pane/slots/browser.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/browser/standalone.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/browser/viewer.rs`
- Modify: `interfaces/webchat/src/components/right_pane/mod.rs` and registry wiring
- Tests: browser parsing/render fallback and existing browser transport tests

- [x] Reuse the existing explicit session/origin props; keep location parsing/scrubbing in `/viewer/browser`, with only compact presentation/status callbacks added.
- [x] Transfer a validated grant once through the existing launcher into `StandaloneBrowserViewer`; do not change `session.rs`, `transport.rs`, `LaunchPhase` or `BrowserWorkspace` ownership.
- [x] Keep “Open in tab” visible, disabled with a reason when authorization is absent. After inline consumption, explicit fallback obtains a fresh grant, closes inline transport, and never automatically reconnects.
- [x] Report slot transport failure to the slot UI and offer the fallback without adding an iframe, Tauri webview, postMessage write-back, or new socket.
- [x] Preserve the viewer’s zero-I/O shell boundary and ensure browser slot unmount closes its transport subscription.
- [x] Add the spec’s source-level census/guard that the slot introduces no second iframe; retain existing canvas iframe behavior.

**Acceptance:** controlled browser content can render in the right slot using existing transport; standalone route and `_blank` fallback remain functional; failure is observable and recoverable. The final offline matrix has 19 executable cases passing; `fullBindingOnlyOwnerInvalidation` remains explicitly blocked because the fail-closed model rejects the public input before the slot can observe it.

## Verification sequence

- [x] Before every Cargo command, run `python qa/browser_live/check_memory.py` per DEVELOPMENT; no concurrent Cargo invocations.
- [x] Run focused tests per stage, then the complete Panel library suite after source settles; report the existing i18n ratchet baseline honestly.
- [x] Run touched-file formatting, Panel check and relevant scoped clippy; avoid whole-workspace formatting of unrelated files.
- [x] Run actual-WASM offline UI checks for toggle, drag bounds, double-click width reset, Canvas/team, flex/legacy and stages 5/6; distinguish these from real Gateway/live stream QA.
- [x] Run the browser existing transport/session tests and the stage-6 source census.
- [x] Inspect `git diff --check`, `git status --short`, and changed-file scope; distinguish any pre-existing failure by exact command and error identity.
- [x] Have an independent reviewer inspect the final diff for duplicate facts, deferred-boundary violations, lifecycle leaks and R1–R10 compliance.

## Explicit non-goals / negative state

- Do not implement TUI right-column changes (stage 7).
- Do not introduce capability producers, runtime capability mount, ACP inbound server, Safe Replay, external-effect exactly-once, provider retry or additional capability backends.
- Do not add a new iframe/webview protocol or write browser state back into `BrowserWorkspace`.
- Do not add draft attachment LRU or replace the existing artifact/session authorities.
- Do not claim visual QA or browser QA passed unless the commands were actually run; if environment dependencies block them, record the exact blocker and leave the task incomplete.
