# S2 可定制壳设计 / S2 customizable shell design

**Status:** Approved 2026-11-23 (D1–D3 resolved as recommended, §5). Parent design: [NSH Panel 增量重构设计](2026-11-22-nsh-panel-modular-refactor-design.md) §4, §6, §7 (S2 row). Roadmap: [migration roadmap](../plans/2026-11-22-nsh-panel-migration-roadmap.md) "S2–S3". Runtime evidence: [S2 runtime spike receipt](../runs/2026-11-23-nsh-panel-s2-runtime-spike.md) (`ec2d264ac`).

## 1. 目标与边界 / Goal and boundaries

S2 交付一份共享 TSX Panel 壳，Desktop（NSH 宿主内嵌 `AppView`）与 Web（NSH Web 包，经 Gateway 同源提供）渲染**同一份源码**，并具备：稳定 ID 布局（排序、显示/隐藏、激活项、恢复默认）及重启后恢复；模块挂载/释放后资源归零；释放后旧 SDK 句柄被拒绝。数据只来自已挂载的只读 Core 方法：`health`、`sessions.list`、`panel.capabilities.snapshot`。

采用已批准的方案 B（保留 Rust-root 宿主，嵌入 `AppView` 作为一个原生组件）。探针已在 macOS 上证明嵌入、props/events、Web 同源渲染与三轮挂载/卸载归零。

**不在 S2 / Out of scope:** 聊天/发送/取消与替代呈现（S3）；snapshot + replay（S4）；逐页迁移 Leptos 页面（S5）；默认切换、发布流水线、Tauri 去留（Phase 11 门禁）；Generated UI；Linux/Windows 的 `AppView` 实测（记为未验证，不由 macOS 代签）。

**English:** S2 ships one TSX shell that Desktop (an embedded `AppView` in the NSH host) and Web (an NSH web package served same-origin by the Gateway) both render from the same source, with persisted stable-ID layout, real teardown, and rejection of disposed SDK handles. It reads only `health`, `sessions.list` and `panel.capabilities.snapshot`. Chat, replay, page migration, default switching and non-macOS runtime evidence stay out.

## 2. 结构 / Structure

```text
interfaces/panel/                      NEW  NSH mixed app (one TSX source)
  nsh.json                                  capabilities.host = ["aleph.rpc"]; web.host = "./web/host.js"
  App.tsx, shell/*.tsx, modules/*.tsx       shell, layout editor, builtin modules
  src/lib.rs   crate aleph-panel-ui         UI set "aleph/panel": exposes shared-ui-logic panel types to TSX
  web/host.js                               browser adapter: /v1/ws framing, ids, hello, connection events
shared/ui_logic/src/panel/             MOVED from interfaces/webchat/src/panel/{registry,projection,discovery,source}.rs
  layout.rs                            NEW  PanelLayout data model + restore/repair
desktop/nsh-shell                      feature `tsx-panel` (off by default)
  panel_surface.rs                     NEW  native component "aleph/panel-surface" wrapping AppView
  panel_host.rs                        NEW  HostChannel handler "aleph.rpc" → AlephClient::connect_local
src/gateway/server/connection/         one shared v1 method set; TLS router serves the web package
```

### 2.1 单一模型源：搬迁而非复制 / Move, don't copy

`interfaces/webchat/src/panel/{registry,projection,discovery,source}.rs` 只依赖 `aleph_protocol`（已核实），是 Leptos 与 TSX 共同需要的读侧索引与投影。它们搬到 `shared/ui_logic` 新的 `panel` feature 下，webchat 改为引用新路径，**旧文件删除，不留转发 shim**。`aleph-panel-ui` 以 `#[class]` / `#[module]` 把同一组类型暴露给 TSX，因此 Desktop（原生）与 Web（wasm）运行的是同一份 Rust 逻辑。

- **代价:** `shared-ui-logic` 已依赖 `aleph-protocol`，新增的只是 `panel` feature；webchat 有一轮 import 改动；TS↔Rust 绑定类型由 NSH 生成，绑定面需保持小。
- **不采用:** 在 TS 中重写 `TabRegistry`/投影——那会产生同一事实的两份表述（判据 1、10）。
- `sdk.rs`、`loader.rs`、`shell.rs`、`capability_surface.rs` 依赖 Leptos，不整体搬迁。计划阶段逐项盘点其中与运行时无关的代际/释放状态机（`sdk.rs` 的 `disposed`、`epoch`、`asset_is_current`）；只有两个渲染器都会调用的部分才抽到 `ui_logic::panel`。

### 2.2 一个 SDK 面，两个宿主适配器 / One SDK surface, two host adapters

TSX 只调用 NSH 的 `host.call("aleph.rpc", { method, params })` 和 `host.on("aleph.connection", …)`。同一 TSX 在两端通过各自的宿主实现：

| 端 | `aleph.rpc` 的实现 | 认证 |
|---|---|---|
| Desktop | `nsh-shell` 在 `AppView::host()` 上注册 handler，转给 `AlephClient::connect_local`（S1-B 的 Unix IPC） | 内核同 euid；仅本机 IPC 目标；不回退 WS |
| Web | `web/host.js`（NSH `web.host`）打开同源 `wss://…/v1/ws`，子协议 `aleph.protocol.v1`，长度前缀帧，首帧 `system.hello` | S1-C 的 HttpOnly `__Host-aleph-v1` Cookie，由浏览器自动携带 |

- 宿主不另设方法白名单：Gateway 的 v1 方法集是唯一授权点（§2.3）；`nsh.json` 的 `capabilities.host` 只授予名字 `aleph.rpc`。
- **代价:** `host.js` 是 JS 写的第二份帧/请求关联实现（判据 10）。限制其职责为帧编解码、id 关联、hello、连接事件，并由 `shared/protocol` 帧测试导出的 golden 向量在 Node 中校验它，防止两边形状漂移。
- **Desktop 远程目标:** S2 的 Desktop TSX 只支持本机 IPC 目标；远程或非本机拥有的目标显示 `unavailable`，不暗中改走 WS。默认 Leptos 路径不受影响。

### 2.3 Gateway：一份 v1 方法集 / One v1 method set

`src/gateway/server/connection/local_ipc.rs:272` 与 `web_v1.rs:447` 各自写了 `matches!(…, "health" | "sessions.list")`，是同一事实的两份表述。S2 把它收为一个共享常量（放在两者都引用的 `connection/` 模块），两个入口都从它派生，并加入只读的 `panel.capabilities.snapshot`。计划需先证明该 handler 经 v1 executor 拿到与旧 `/ws` 相同的调用者与项目可见性（`src/gateway/handlers/panel_capabilities.rs` 使用 `PanelRequestContext`）；若拿不到，记为缺口，先补上下文，不放宽可见性。`panel.project.select` 会改连接状态，不在 S2 加入。

### 2.4 Desktop 嵌入 / Desktop embedding

- **Feature 闸:** `aleph-nsh-shell` 新增 `tsx-panel` feature（默认关闭），打开 `nsh-platform-*` 的 `js`，并链接 `aleph-panel-ui`。默认构建、Phase 11 的证据与发版路径不变。体积与构建时间变化写入回执。
- **可信 bundle:** `Bundle::source(include_str!(<build-time path>))`，即 bundle 随 app 二进制签名，不从可写路径读取（回执结论 2）。bytecode（`read_trusted`）推迟。bundle 由 `just` 目标在 cargo 前用 `nsh build interfaces/panel` 生成；缺 bundle 时编译失败并给出构建命令，不静默退回。
- **选择渲染器:** 启动参数/环境 `ALEPH_SHELL_PANEL=tsx` 时主窗口挂 `native("aleph/panel-surface")`，否则仍是现有 `web_view`。两者不同时运行。
- **组件:** `panel_surface.rs` 的 `NativeComponent` 持有 `Rc<AppView>`，`Drop` 时 `stop()`，移除 host handler，关闭 IPC 连接。AppKit 工作只放在 factory 与组件中（回执结论 3）。注册是进程级的（结论 4），因此 factory 在已有存活 surface 时拒绝创建第二个，只记录日志并渲染空视图，防止嵌套。
- **Linux/Windows:** 同一代码经 `platform.rs` 编译，但 `AppView` 嵌入只在 macOS 有证据；其他平台在回执中标为未验证。

### 2.5 Web 提供 / Web serving

TLS 路由（与 `/v1/session`、`/v1/ws` 同源）新增 `/v1/panel/`，用已有依赖 `tower-http` 的 `ServeDir`，从配置项指定的目录提供 `nsh package --target web` 的输出；未配置时路由不挂载（404）。响应带 CSP `script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'`（回执结论 7）。不把约 7.5 MB 的包嵌进 `aleph-server`（R3），嵌入与否在发布阶段另议。明文与 wrapper 路由不挂载。

浏览器首次进入时没有 Cookie：壳显示一个最小配对表单，用户粘贴一次性配对票据，`host.js` 以同源 `fetch` POST `/v1/session`。票据不落存储、不进 URL；Cookie 仍是 HttpOnly，脚本读不到。

### 2.6 布局持久化 / Layout persistence

`ui_logic::panel::layout::PanelLayout { version: 1, order, hidden, active }` 只含 `PanelTabKey` 与纯数据。`restore(raw, &TabRegistry)`：版本未知或解析失败时恢复默认并给出修复说明；快照中已不存在的键保留为 `unavailable` 占位，不执行；新出现的键追加到末尾。存储走 NSH `Storage`（Desktop：设备上的 app 数据；Web：该 app 前缀下的 `localStorage`），键为 `aleph.panel.layout.v1:<endpoint>:<subject>`。损坏只重置表现，不碰会话或插件。

`subject` 来自 `HelloResult` 新增的可选不透明字段（§5 D1）；当前 `HelloResult` 不含主体信息（`shared/protocol/src/system_hello.rs`）。

### 2.7 模块生命周期与 SDK / Module lifecycle and SDK

- 壳按 qualified key 挂载模块。每个模块拿到绑定 `(key, generation)` 的 `PanelSdk`：`invoke`、`subscribe`、`connection`、`navigate`、`theme`、`locale`、`dispose`，与 webchat `PanelSdk` 同名同义。
- `dispose()` 推进代际：之后的 `invoke` 以 `disposed` 拒绝，迟到结果丢弃，`host.on` 监听移除；隐藏模块即释放它，不停用后端。
- 连接断开、目标切换或快照 revision 变化时同样推进代际。Core 仍按请求重新准入；客户端代际只防止旧回调提交，不是安全边界。
- 资源归零检查必须等待引擎静止（回执结论 5），不在卸载后立即采样。

### 2.8 S2 的内置模块 / Builtin modules

| 模块 | 数据 | 说明 |
|---|---|---|
| `aleph:overview` | `health` | 连接与服务状态 |
| `aleph:sessions` | `sessions.list` | 只读列表；打开会话属于 S3 |
| `aleph:capabilities` | `panel.capabilities.snapshot` | 目录与状态；只有 DOM 挂载 ABI 的社区 tab 显示为「此渲染器不可用」，不执行、不猜测转换 |

## 3. 验证 / Acceptance

- **单元:** 搬迁后的 registry/projection/discovery 测试原样通过；`PanelLayout` 的损坏/未知版本/缺失键/新增键；方法集单一来源（两入口拒绝同一未挂载方法）。
- **Gateway:** `panel.capabilities.snapshot` 经 IPC 与 `/v1/ws` 返回与旧 `/ws` 相同的、按调用者过滤的结果；`/v1/panel/` 只挂在 TLS 路由、带 CSP、未配置时 404。
- **`host.js`:** Node 用 golden 帧向量测试编解码、乱序响应、超限帧。
- **Desktop 真机（macOS）:** `tsx-panel` 构建的 `nsh-shell` 连隔离 QA Gateway 的 IPC：三个模块显示真实数据；排序/隐藏后重启恢复；隐藏模块后其订阅与计时器归零；保留的旧 SDK 调用被拒绝；多轮挂载线程回到基线。
- **Web 真机（Chrome）:** TLS fixture 上完成配对 → `/v1/panel/` → 同一 TSX 显示同样三个模块；重新加载后布局恢复；CSP 响应头存在。
- **同源证明:** 回执记录 `interfaces/panel` 的源码树哈希，以及由它生成的 Desktop 内嵌 bundle 与 Web 包。
- **回归:** 默认（非 `tsx-panel`）`nsh-shell` 与 webchat 构建和测试不变；每次 cargo 前做内存门控，与既有验证纪律一致。

## 4. 清理位置 / Cleanup locations

- `interfaces/webchat/src/panel/{registry,projection,discovery,source}.rs`：搬迁后删除。
- `local_ipc.rs:272` 与 `web_v1.rs:447` 的重复 `matches!`：由共享方法集取代。
- `qa/nsh_s2_spike/`：S2 真机 QA 取代其证据后删除，回执改为引用提交哈希。
- Leptos 壳与页面不在 S2 删除：仍是默认渲染器，按 S5/S6 台账退场。

## 5. 已定决策 / Resolved decisions (2026-11-23)

- **D1 布局主体键：** `HelloResult` 新增可选的不透明 `subject`，由 Gateway 从已绑定主体派生（服务端密钥 HMAC，不暴露用户 ID），旧客户端忽略。
- **D2 Web 包提供方式：** 配置目录 + `ServeDir`，默认不挂载；不嵌入 `aleph-server`。
- **D3 浏览器配对表单：** S2 内提供最小配对表单。
