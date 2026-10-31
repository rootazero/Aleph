# 嵌入终端（纯 PTY） / Embedded terminal (pure PTY)

本文件描述当前终端能力。原先的 herdr 风格运行时 agent 探测、状态仲裁、观察工具和 agent 面板已经移除；历史设计与实现记录保留在 `docs/superpowers/` 中，不再作为现行架构契约。

## 当前边界

终端事实由服务端 PTY 与 VT screen 持有：`src/gateway/pty/` 负责 spawn、输入、resize、attach、screen frame、OSC 7 cwd、title 和退出事件。客户端只消费 `pty.list`、`pty.screen`、`pty.exit`，维护自己的 tab 选择与 canvas/render 状态。

保留的主要入口：

- RPC：`pty.spawn`、`pty.list`、`pty.attach`、`pty.input`、`pty.resize`、`pty.close`
- 事件：`pty.screen`、`pty.exit`
- 协议：`shared/protocol/src/pty.rs`
- Gateway handler：`src/gateway/handlers/pty.rs`
- Webchat：`interfaces/webchat/src/platform/wide/views/terminal/`
- TUI：终端 tabs、PTY 事件、输入、resize 和 attach 路径

`pty.attach` 仍执行 session ownership 校验并返回 attach snapshot。客户端卸载不会销毁服务端 screen；重新 attach 可以恢复当前 screen。退出事件只驱动客户端清理 tab，不会创建新的 shell。

WebSocket 重连后，Web terminal 会重新订阅临时的 `pty.screen` 与 `pty.exit` 事件，并重新 attach 当前选中的 session；服务端 screen 状态因此可以在连接恢复后继续显示。
After a WebSocket reconnect, the web terminal re-subscribes to the ephemeral `pty.screen` and `pty.exit` events and re-attaches the selected session, so the server-side screen can continue rendering after the connection is restored.

## Provider lifecycle / Provider 生命周期

The existing `PtyManager` owns one pure `ProviderHandle` and one joinable flush driver alongside the canonical sessions, retained owners and bus. `Config::policies.terminal.enabled` is the sole enabled intent. Disabled router boot does not initialize the manager, attach the bus, spawn the driver or create sessions. Enabled activation owns a driver before `Ready`. Stop revokes admission before checked close-all, signals and joins the retained driver; failure retains typed evidence and failed sessions, and cancellation preserves the handle. Operations and screen/exit publication/forwarding check the captured Ready generation; re-enable with the existing bus receives a new generation. `created_by` ownership and the existing process journal remain unchanged.

现有 `PtyManager` 在 canonical session、retained owner 与 bus 之外持有一个纯 `ProviderHandle` 和一个可 join 的 flush driver。`Config::policies.terminal.enabled` 是唯一 enabled intent。Disabled router boot 不初始化 manager、不 attach bus、不 spawn driver、不创建 session。Enabled activation 先拥有 driver 再发布 `Ready`。Stop 在 checked close-all 前撤销 admission，signal 并 join 保留的 driver；失败保留 typed evidence 与失败 session，取消保留 handle。操作与 screen/exit publication/forwarding 校验捕获的 Ready generation；已有 bus 的 re-enable 获得新 generation。`created_by` ownership 与既有 process journal 保持不变。

The synchronous live-apply API reports pending join or failure as non-live/restart and retains ownership for later apply/start or async shutdown. Cold disabled boot has no bus, so enabling from that state also truthfully needs restart. File-watcher updates alone do not execute this explicit apply path. Normal shutdown awaits stop; forced shutdown bounds its join attempt to one second, which cannot preempt synchronous native I/O. Observed kill return and flush-driver join do not prove native unload, OS child/reader-thread/RSS reclamation or process isolation. Terminal Zahir/BackendLease/MCP/ACP projection and durable Capability recovery remain deferred. Exact validation and limitations: [Terminal lifecycle run](../superpowers/runs/2026-10-31-terminal-lifecycle-run.md).

同步 live-apply 对 pending join 或失败报告 non-live/restart，并保留所有权供后续 apply/start 或 async shutdown 收尾。Cold disabled boot 没有 bus，从该状态 enable 也如实需要 restart。仅文件 watcher 更新不执行此显式 apply path。正常 shutdown 等待 stop；forced shutdown 的 join 尝试限制为一秒，但无法抢占同步 native I/O。kill 返回与 flush-driver join 不证明 native unload、OS child/reader-thread/RSS reclamation 或 process isolation。Terminal Zahir/BackendLease/MCP/ACP projection 与 durable Capability recovery 仍 deferred。精确验证与限制见上述 run record。

## Panel module lifecycle / Panel 模块生命周期

The standalone terminal entry owns only its UI resources: the SDK polling task, the two DOM roots, the Leptos `Owner`, PTY event subscriptions, resize/observer callbacks, animation frames, and the loaded module handle. `mount` creates the local executor before the SDK adapter starts its connection polling, then creates one sidebar surface and one Canvas surface. `unmount` is idempotent and releases those resources; it clears both roots and never calls `pty.close`.

独立 Terminal module 只拥有 UI 资源：SDK polling task、两个 DOM root、Leptos `Owner`、PTY 事件订阅、resize/observer 回调、animation frame 和已加载 module handle。`mount` 会先初始化浏览器 `spawn_local` executor，再创建 SDK adapter，最后创建 sidebar 与 Canvas 两个 surface。`unmount` 幂等释放这些资源并清空两个 root；它绝不调用 `pty.close`。

The PTY/session lifetime belongs to the Gateway. Leaving the route, disabling the tab, replacing a module, or disposing a UI owner does not close the server session. Only an explicit user action routed to the PTY close operation may call `pty.close`; a test may call it only for the disposable session that the same test created. Source-owned capacity eviction, provider disable and daemon shutdown can also end sessions without a UI close RPC. A later visit reuses a surviving selected session through `pty.list` and `pty.attach`.

PTY/session 生命周期属于 Gateway。离开路由、禁用 Tab、替换 module 或释放 UI owner 都不会关闭服务端 session。只有经过 PTY close operation 的明确用户动作才能调用 `pty.close`；测试只能为本次测试自己创建的 disposable session 显式调用它。Source-owned capacity eviction、provider disable 与 daemon shutdown 也可结束 session，不需要 UI close RPC。再次进入时通过 `pty.list` 与 `pty.attach` 复用仍存活的 selected session。

The host-owned source/snapshot/projection path is the boundary between capability facts, route selection, and module loading. The serializable `PanelUiCapabilityDeclaration`, `PanelUiCapabilityOrigin`, and `PanelUiCapabilitySnapshot` live in `shared/protocol/src/panel.rs`; `interfaces/webchat/src/panel/source.rs` provides source implementations and built-in/fixture construction. Core derives the explicit community discovery snapshot in `src/extension/panel.rs`, stores the complete replacement snapshot in `src/extension/projection.rs`, and recomputes only from `src/extension/lifecycle.rs::after_transition`. The optional `panel` manifest section is explicit and is never inferred from generic plugin entry, kind, permissions, `capabilities_v2`, or `wasm_capabilities`. `interfaces/webchat/src/panel/projection.rs::PanelProjection::{from_snapshot,replace_snapshot}` validates every declaration with stable fail-closed reason codes and atomically accepts only a newer valid snapshot. Community entries require `./tabs/<plugin_id>/<plugin_version>/<file>` plus SRI-shaped sha256 integrity and an explicit supported public base. The validated optional integrity travels through the existing projection → `TabRegistry` → `PanelLoader` path. `PanelTabDescriptor` remains the only browser projection shape; `PanelShell` and `PanelLoader` remain browser-side consumers. Discovery contract evidence and deferred boundaries are recorded in [panel-community-manifest-discovery-validation.md](../reports/panel-community-manifest-discovery-validation.md).

The production discovery bridge now reuses the authenticated `/ws` WebSocket JSON-RPC control plane. `panel.capabilities.snapshot` is registered through the existing handler registry and reads the canonical Core snapshot once; webchat queries it after authorized connect and reconnect into a separate discovery state with explicit error/disconnected status and epoch/revision fencing. This does not connect community declarations to route selection, `PanelShell`, `PanelLoader`, asset fetch, or module import. EventBus live notifications remain deferred because its broadcast path is lossy and advisory; reconnect and explicit refresh use the complete snapshot query for recovery. Install/update/uninstall, multi-file graphs, external WASM, and live Gateway wiring remain outside this stage.

For the constrained community phase, `PanelAssetPublicationStore` is the narrow in-process publication seam for exact bytes already admitted by canonical host state. It accepts only the canonical identity, versioned `./tabs/` entry, trusted/enabled community declaration, exact SHA-256 integrity and bytes; it never accepts a plugin root, filesystem path, URL, manifest or generic plugin field. The current phase deliberately leaves live HTTP route mounting deferred because an authoritative byte producer and browser credential injection have not been approved. The deterministic `panel-tab-spike` adapter is local and explicitly does not provide Gateway parity. `/panel/<plugin_id>/<tab_id>` is the only community selection shape; snapshot arrival remains passive, and the existing `PanelShell`/`PanelLoader` owns generation, descriptor, integrity and one-shot cleanup fencing.

对于受约束的 community 阶段，`PanelAssetPublicationStore` 是一个窄的进程内 publication seam，负责提供 canonical host state 已准入的 exact bytes。它只接受 canonical identity、versioned `./tabs/` entry、trusted/enabled community declaration、精确 SHA-256 integrity 与 bytes；绝不接受 plugin root、filesystem path、URL、manifest 或 generic plugin field。本阶段明确 deferred live HTTP route mounting，因为权威 bytes producer 与 browser credential injection 尚未获批。deterministic `panel-tab-spike` adapter 只在本地工作，不能宣称 Gateway parity。`/panel/<plugin_id>/<tab_id>` 是唯一 community selection shape；snapshot arrival 保持 passive，现有 `PanelShell`/`PanelLoader` 继续负责 generation、descriptor、integrity 与 one-shot cleanup fencing。

host-owned source/snapshot/projection 路径是 capability facts、route selection 与 module loading 之间的边界。可序列化的 `PanelUiCapabilityDeclaration`、`PanelUiCapabilityOrigin` 与 `PanelUiCapabilitySnapshot` 位于 `shared/protocol/src/panel.rs`；`interfaces/webchat/src/panel/source.rs` 提供 source 实现以及 built-in/fixture 构造。Core 在 `src/extension/panel.rs` 派生显式 community discovery snapshot，在 `src/extension/projection.rs` 整体保存 replacement snapshot，并且只由 `src/extension/lifecycle.rs::after_transition` 重算。可选 `panel` manifest section 必须显式声明，绝不从通用 plugin entry、kind、permissions、`capabilities_v2` 或 `wasm_capabilities` 推断。`interfaces/webchat/src/panel/projection.rs::PanelProjection::{from_snapshot,replace_snapshot}` 用稳定 reason code 对每个 declaration fail-closed 验证，并且只原子接受更新且完全有效的 snapshot。Community entry 必须是 `./tabs/<plugin_id>/<plugin_version>/<file>`，并带 SRI 形状的 sha256 integrity 与显式受支持的 public base。已验证的 optional integrity 沿现有 projection → `TabRegistry` → `PanelLoader` 路径传递；`PanelTabDescriptor` 仍是唯一 browser projection shape；`PanelShell` 与 `PanelLoader` 仍是浏览器侧消费者。discovery contract 证据与延后边界见 [panel-community-manifest-discovery-validation.md](../reports/panel-community-manifest-discovery-validation.md)。

生产 discovery bridge 复用已认证的 `/ws` WebSocket JSON-RPC control plane。`panel.capabilities.snapshot` 通过现有 handler registry 注册，并且每次只读取一次 Core canonical snapshot；webchat 在授权连接和重连后把结果写入独立 discovery state，并显式保留 error/disconnected 状态以及 epoch/revision fencing。社区 declaration 仍不会进入 route selection、`PanelShell`、`PanelLoader`、asset fetch 或 module import。由于 EventBus 广播路径可丢失且只具 advisory 语义，live notification 仍 deferred；reconnect 与 explicit refresh 通过完整 snapshot query 恢复。install/update/uninstall、多文件 graph、external WASM 与 live Gateway wiring 仍不在本阶段。

Each SDK connection snapshot has an epoch. A reconnect replaces the old `pty.screen` and `pty.exit` subscriptions, re-attaches the selected session, and refreshes from the new epoch. Async list, attach, subscribe, and event results must check the active owner and connection epoch before changing the UI. Results from a disposed owner or an old epoch are discarded; they must not spawn a fallback PTY or report a false success.

每个 SDK connection snapshot 都带有 epoch。重连会释放旧的 `pty.screen`/`pty.exit` 订阅，在新 epoch 下重新 attach selected session，并从新 epoch 刷新。异步 list、attach、subscribe 与 event 结果在写入 UI 前必须检查 active owner 和 connection epoch；已释放 owner 或旧 epoch 的结果会被丢弃，不得 fallback spawn PTY，也不得报告假成功。

## 已移除代码的归档 / Archive for removed code

已移除的终端 agent 与模型路由代码归档在仓库根目录的 `archive/` 下：`archive/removed-terminal-agent-moa/` 和 `archive/removed-model-routing-moa/`。现行文档仍放在 `docs/`；归档代码不构成当前产品契约。
Removed terminal-agent and model-routing code is archived under the repository root `archive/`: `archive/removed-terminal-agent-moa/` and `archive/removed-model-routing-moa/`. Current documentation remains under `docs/`; archived code is not a current product contract.

## 已移除的能力

以下路径不再属于产品或 workspace：

- `crates/agent-detect/` 及其 Apache-2.0 派生识别引擎
- `src/gateway/pty/foreground.rs` 的前台进程探测
- `src/gateway/runtime/` 与 `RuntimeAgents` 状态表
- `runtime.agents.list`、`runtime.agents.changed` 与 `terminal_sessions_*` 观察面
- `src/builtin_tools/terminal.rs` 及 observation capability registry
- Webchat/TUI runtime agent panel、共享 `agent_panel` 状态和相应 locale
- herdr/agent runtime QA 驱动与 fixtures

因此，`pty.list` 只报告 PTY session 本身的协议字段：session id、shell、cwd、创建时间和 closed 状态。它不推断其中运行的程序、agent、工作状态或 quiet 时间。

## 变更规则

增加新的终端事实时，先在 `shared/protocol/src/pty.rs` 定义 wire 类型，再由 server screen/session 派生并通过 `pty.screen` 或 attach snapshot 传出；前端只渲染和维护本地选择。不要重新引入进程表读取、agent manifest 识别、运行时状态广播或面板专用观察 RPC。

VT 行为继续扩展 `src/gateway/pty/screen/` 的唯一实现。终端视图保持纯渲染边界；Channel、CLI 和 Panel 不承载终端业务判断。

## 验证

修改核心终端后至少运行：

```text
cargo check --workspace
cargo test -p alephcore --lib gateway::pty
cargo test -p aleph-panel --lib
```

Panel host acceptance additionally runs from `interfaces/webchat`:

```text
node tests/panel-terminal-protocol.test.mjs
node tests/panel-terminal-host.test.mjs
```

The first is the deterministic component fixture; the second boots the staged host WASM and real Panel loader/SDK against a loopback Gateway-shaped HTTP/WS fixture. Neither replaces the separate real Gateway/live smoke test, which must remain explicitly marked BLOCKED when no disposable Gateway target is available. The build-aware host fixture must receive `PANEL_OUTPUT_ROOT` from the clean source build; it rejects an unset root and therefore cannot silently consume the historical ignored `.panel-tabs` directory. The 2026-10-28 validation generated a candidate from revision `3ac50c1be5c3a613195cb76c080ef7d93f99d9ba` and exercised the final staged HTML and versioned Terminal assets through the production host. The result is recorded in [`panel-terminal-build-e2e-validation.md`](../reports/panel-terminal-build-e2e-validation.md); this remains loopback evidence, and the separate live Gateway verdict remains BLOCKED without an authorized disposable target.

build-aware host fixture 必须接收 clean source build 生成的 `PANEL_OUTPUT_ROOT`；未设置时直接拒绝，因此不会静默消费历史 ignored `.panel-tabs` 目录。2026-10-28 验证从 revision `3ac50c1be5c3a613195cb76c080ef7d93f99d9ba` 生成 candidate，并由 production host 加载最终 staged HTML 与 versioned Terminal assets。结果记录在 [`panel-terminal-build-e2e-validation.md`](../reports/panel-terminal-build-e2e-validation.md)；这仍是 loopback evidence，没有授权 disposable target 时独立 live Gateway verdict 仍为 BLOCKED。

The generic host boundary delivery is recorded in [`2026-10-27-panel-shell-projection-delivery.md`](../superpowers/plans/2026-10-27-panel-shell-projection-delivery.md). This stage adds only a controlled, explicitly trusted, self-contained community ES module fixture to the existing loader. It does not claim a second built-in Tab, community install/update/uninstall, production discovery, arbitrary multi-file JS/WASM loading, production ExtensionManager or ZahirFacade mounting, live Gateway parity, a safe same-origin sandbox, or a default release flip.

Panel host acceptance 还应在 `interfaces/webchat` 下运行上述两个 fixture：前者是确定性 component fixture，后者启动 staged host WASM 与真实 Panel loader/SDK，并连接 loopback Gateway 形状的 HTTP/WS fixture。两者都不能替代独立的真实 Gateway/live smoke test；没有 disposable Gateway target 时必须明确标记为 BLOCKED。

真机 QA 只覆盖仍存在的 PTY spawn、resize、attach、input、screen 和 exit 行为；已删除的 agent detection/runtime/panel 阶段不应重新加入 QA 路由。
