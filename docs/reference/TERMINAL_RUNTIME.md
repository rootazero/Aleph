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

## Panel module lifecycle / Panel 模块生命周期

The standalone terminal entry owns only its UI resources: the SDK polling task, the two DOM roots, the Leptos `Owner`, PTY event subscriptions, resize/observer callbacks, animation frames, and the loaded module handle. `mount` creates the local executor before the SDK adapter starts its connection polling, then creates one sidebar surface and one Canvas surface. `unmount` is idempotent and releases those resources; it clears both roots and never calls `pty.close`.

独立 Terminal module 只拥有 UI 资源：SDK polling task、两个 DOM root、Leptos `Owner`、PTY 事件订阅、resize/observer 回调、animation frame 和已加载 module handle。`mount` 会先初始化浏览器 `spawn_local` executor，再创建 SDK adapter，最后创建 sidebar 与 Canvas 两个 surface。`unmount` 幂等释放这些资源并清空两个 root；它绝不调用 `pty.close`。

The PTY/session lifetime belongs to the Gateway. Leaving the route, disabling the tab, replacing a module, or disposing a UI owner does not close the server session. Only an explicit user action routed to the PTY close operation may call `pty.close`; a test may call it only for the disposable session that the same test created. A later visit reuses the selected session through `pty.list` and `pty.attach`.

PTY/session 生命周期属于 Gateway。离开路由、禁用 Tab、替换 module 或释放 UI owner 都不会关闭服务端 session。只有经过 PTY close operation 的明确用户动作才能调用 `pty.close`；测试只能为本次测试自己创建的 disposable session 显式调用它。再次进入时通过 `pty.list` 与 `pty.attach` 复用 selected session。

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

The first is the deterministic component fixture; the second boots the staged host WASM and real Panel loader/SDK against a loopback Gateway-shaped HTTP/WS fixture. Neither replaces the separate real Gateway/live smoke test, which must remain explicitly marked BLOCKED when no disposable Gateway target is available.

Panel host acceptance 还应在 `interfaces/webchat` 下运行上述两个 fixture：前者是确定性 component fixture，后者启动 staged host WASM 与真实 Panel loader/SDK，并连接 loopback Gateway 形状的 HTTP/WS fixture。两者都不能替代独立的真实 Gateway/live smoke test；没有 disposable Gateway target 时必须明确标记为 BLOCKED。

真机 QA 只覆盖仍存在的 PTY spawn、resize、attach、input、screen 和 exit 行为；已删除的 agent detection/runtime/panel 阶段不应重新加入 QA 路由。
