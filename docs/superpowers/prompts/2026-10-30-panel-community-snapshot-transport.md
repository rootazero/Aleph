# Panel community snapshot transport — next-stage restart prompt

实施 Panel 社区 UI 的下一阶段：**把已经完成的 Core canonical Panel capability snapshot 通过现有 JSON-RPC/WebSocket control plane 送达 webchat，建立可审计的 production discovery transport bridge；本阶段不进入 asset publication、浏览器生产挂载或安装生命周期。**

## 已完成基线 / Completed baseline

本阶段已合并进 `main`，当前基线 commit 为 `ba3c64c1d`：

- `shared/protocol/src/panel.rs` 是唯一可序列化的 `PanelUiCapabilityDeclaration`、`PanelUiCapabilityOrigin`、`PanelUiCapabilitySnapshot` owner。
- `src/extension/manifest` 解析显式可选 `panel` declaration；generic plugin `entry`、kind、permissions、`capabilities_v2` 与 `wasm_capabilities` 不产生 Panel row。
- `src/extension/panel.rs` 从 admitted `PluginRecord`、`OwnerTrustPolicy` 与 host lifecycle state 派生 canonical snapshot。
- `src/extension/projection.rs` 与 `src/extension/lifecycle.rs::after_transition` 是唯一的 snapshot revision/retraction authority；snapshot 自身同时携带 revision。
- `interfaces/webchat/src/panel/projection.rs` 已能消费并验证 canonical snapshot，但 production `app.rs` 仍由 built-in/fixture source 初始化；没有 Core → webchat 的 production transport bridge。
- integrity-gated single-file loader、Terminal host、mount/unmount cleanup 和 late-result fencing 已完成；不要重新实现 loader。

已知验证限制：Node loader regression 通过；manifest focused test 的 alephcore test binary link 仍受 Windows LLVM OOM 限制；Panel artifact build 在本 checkout 因缺少本地 `tailwindcss` script 停止。详细证据见 `docs/reports/panel-community-manifest-discovery-validation.md`。

## 本阶段目标 / Goal

通过现有 JSON-RPC/WebSocket control plane 暴露并消费同一个 `PanelUiCapabilitySnapshot`：

1. Core/ExtensionManager 仍是 snapshot authority；server transport 只读取它，不复制 registry、trust policy 或 revision counter。
2. transport response 与 notification 直接使用 shared protocol snapshot shape，不增加第二个 Panel DTO。
3. webchat 能在连接建立、重连和显式刷新时获得最新 snapshot，并按 revision 丢弃 stale snapshot。
4. transport 的错误、断连、旧 epoch 与 revision mismatch fail closed；不得把错误解释成“没有社区 Panel”。
5. 本阶段只证明 discovery transport 到达了 webchat discovery state，不把 community descriptor 接入 `PanelShell`、route selection 或 `PanelLoader`。

## 明确不做 / Explicitly out of scope

- 不实现 asset publication/server、public hosting 或 manifest entry 到实际文件的发布映射。
- 不增加 production browser route、`PanelShell` community mount、`app.rs` community route 或 module import。
- 不实现 install/update/uninstall、marketplace、signature service、background refresh 或 default release change。
- 不修改 ZahirFacade runtime mount、`src/harness/`、PTY/Gateway business logic 或其他 plugin lifecycle。
- 不引入第二个 Panel registry、descriptor shape、permission model、trust model、snapshot authority 或 revision source。
- 不把 local fixture、loopback Gateway 或 fake JSON-RPC 当成 live Gateway evidence。

## 起点与工作纪律 / Starting point and workflow

- 新实现从当前 `main` 的 `ba3c64c1d` 创建独立 worktree/branch，建议分支名 `panel-community-transport`。
- 不直接编辑共享 `main`；本阶段默认不 merge、不 push。
- 使用 `superpowers:executing-plans` 顺序执行：
  `docs/superpowers/plans/2026-10-30-panel-community-snapshot-transport-plan.md`。
- 测试先写最小 RED，确认失败原因后实现最小局部修改；每个交付块保留实际验证证据并使用 conventional commit。
- 每次 Cargo subprocess 前，从 worktree root 执行 `python qa/browser_live/check_memory.py`，通过后从 owning package directory 串行运行 Cargo。
- 中文沟通，English code comments，双语文档；仓库文档不得写机器绝对路径。

## Read first / 先读

1. `AGENTS.md`、`docs/reference/REDLINES.md`、`docs/reference/ARCHITECTURE.md`、`docs/reference/FEATURE_LOCATOR.md` §6.11、`docs/reference/TERMINAL_RUNTIME.md`。
2. `docs/superpowers/plans/2026-10-29-panel-community-manifest-discovery-plan.md` 与 `docs/reports/panel-community-manifest-discovery-validation.md`。
3. Shared protocol snapshot：`shared/protocol/src/panel.rs`。
4. Core authority：`src/extension/panel.rs`、`src/extension/projection.rs`、`src/extension/lifecycle.rs`、`src/extension/mod.rs`。
5. Existing control plane：Gateway JSON-RPC router/handler registration、现有 snapshot/query/event response patterns，以及 client reconnect/epoch handling。
6. Webchat consumer：`interfaces/webchat/src/panel/{source,projection,registry,loader,shell}.rs`、`interfaces/webchat/src/app.rs`、对应 Rust/Node fixtures。
7. 先完成依赖与 ownership audit，再选择具体 RPC method/event name。不要把 `PluginManifest`、Core type 或 `ExtensionManager` 引入 WASM。

## Required outcome / 必须产出

- 一个通过现有 control plane 的 read-only snapshot query；如果需要新增 JSON-RPC method，必须复用现有 WebSocket endpoint、router、auth 与 error envelope，不得新增 HTTP/WS endpoint。
- 可选的 snapshot change notification 只有在现有 event/subscription seam 已被证明为正确 owner 时才添加；否则采用 reconnect/explicit refresh，并在报告中记录 live update seam deferred。
- response/notification payload 直接是 `PanelUiCapabilitySnapshot`，不复制声明字段。
- webchat discovery state 保存最新 canonical snapshot、连接 epoch 与 last accepted revision；旧 epoch、旧 revision、malformed payload、transport error 均 fail closed。
- local fixture 证明：Core-shaped canonical snapshot → existing control-plane envelope → webchat decode/discovery state；fixture 不得调用 `PanelProjection::from_snapshot` 去制造生产 mount 证据。
- 文档与验证报告记录 transport owner、revision/epoch/reconnect semantics、错误处理和仍然延后的 asset/browser/install boundaries。

## Acceptance / 验收标准

- generic plugin 或 manifest 外字段不能通过 transport 产生 Panel declaration。
- transport 只读取 `ExtensionManager::panel_ui_capability_snapshot()`，没有第二个 registry、revision counter 或 trust decision。
- webchat 能区分 `Ok(empty snapshot)`、transport error、decode error、stale snapshot 和 disconnected state。
- snapshot revision 单调接受；旧 snapshot 不得覆盖新 snapshot；reconnect 可用完整 snapshot 恢复。
- 没有 production `PanelShell` community mount、route selection、loader import 或 asset fetch。
- Terminal/built-in source、existing loader tests 与 shared protocol round-trip 保持通过。
- 最终报告精确区分：transport contract 已证明；asset publication、browser mount、install lifecycle 和真实 Gateway parity 仍 deferred/blocked。

English handoff: continue from `main` at `ba3c64c1d` in a fresh `panel-community-transport` worktree. Add a read-only production snapshot bridge over the existing JSON-RPC/WebSocket control plane and a webchat discovery consumer using the canonical shared `PanelUiCapabilitySnapshot`. Keep Core as the only snapshot authority, use revision and connection-epoch fencing, and stop before asset publication, browser mount, installation lifecycle, default-release changes, or live Gateway claims.
