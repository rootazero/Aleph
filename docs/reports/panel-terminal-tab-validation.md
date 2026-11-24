# Panel Terminal Tab validation / Panel Terminal Tab 验收记录

## Scope / 范围

This report records the bounded Terminal host acceptance follow-up from the `terminal-host` worktree. It keeps the earlier modular migration measurements and deterministic protocol evidence, then adds production-host/wire evidence through the staged host WASM, real `PanelShell`/`PanelSdk`, versioned terminal module, and a loopback Gateway-shaped HTTP/WS fixture. Real Gateway parity remains a separate disposition.

本报告记录 `terminal-host` worktree 中 Terminal host acceptance 的有界后续工作。它保留此前模块化迁移的测量与确定性协议证据，并新增通过 staged host WASM、真实 `PanelShell`/`PanelSdk`、版本化 terminal module 以及 loopback Gateway 形状 HTTP/WS fixture 的 production-host/wire 证据。真实 Gateway parity 仍单独记录结论。

The current validation branch starts at `8b8a70385` (`docs(panel): plan terminal host acceptance`), matching `origin/main` at worktree creation. The default legacy build remains the rollback path; `panel-modular` remains opt-in.

当前验收分支从 `8b8a70385`（`docs(panel): plan terminal host acceptance`）开始，创建 worktree 时与 `origin/main` 一致。默认 legacy 构建仍是回退路径；`panel-modular` 仍为 opt-in。

## Deterministic browser protocol fixture / 确定性浏览器协议 fixture

Fixture: `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`.

Command:

```text
cd interfaces/webchat
node tests/panel-terminal-protocol.test.mjs
```

Result: passed for both `/` and `/panel/` public bases. The fixture serves and loads the final `.panel-tabs` entry `./tabs/terminal/ff172f356de7e061/terminal.js` and its `terminal_bg.wasm`; it does not use the old pilot artifact.

结果：root `/` 与 `/panel/` 两种 public base 均通过。fixture 从最终 `.panel-tabs` staging 资产加载 `./tabs/terminal/ff172f356de7e061/terminal.js` 和 `terminal_bg.wasm`，没有使用旧 pilot artifact。

The protocol fixture asserts observable screen state and recorded host RPC/subscription events:

- Chat startup does not request the terminal module or mount terminal UI.
- First Terminal navigation loads the versioned JS/WASM once and creates the sidebar and Canvas roots; returning after Chat and Settings reuses the module and leaves one active pair of listeners.
- `pty.list` refusal is rendered as an error and never falls through to `pty.spawn`.
- `pty.input` preserves Ctrl-C byte `0x03`, Unicode UTF-8, and the bracketed-paste wrapper; `pty.resize` carries positive geometry.
- Only exact `pty.screen` and `pty.exit` topics are subscribed. A sequence gap causes attach recovery and does not report a false success.
- A connection epoch change disposes old subscriptions and re-attaches through the new epoch. Late list/attach results cannot repopulate cleared roots after quick switch, disable/re-enable, or width changes.
- UI teardown does not call `pty.close`; the fixture has no disposable real PTY to close.
- Missing or rejected terminal operations affect the Terminal surface while Chat and Settings remain navigable.

fixture 断言可见屏幕状态以及 host RPC/订阅事件：

- Chat 启动不会请求 terminal module，也不会 mount terminal UI。
- 首次进入 Terminal 只加载一次版本化 JS/WASM 并创建 sidebar 与 Canvas root；经过 Chat 和 Settings 返回时复用 module，并保持一对 active listener。
- `pty.list` 被拒绝时显示错误，绝不会 fallback 到 `pty.spawn`。
- `pty.input` 保留 Ctrl-C 的 `0x03` 字节、Unicode UTF-8 与 bracketed-paste wrapper；`pty.resize` 携带正数 geometry。
- 只订阅 exact `pty.screen` 与 `pty.exit` topic。sequence gap 会触发 attach recovery，不报告假成功。
- connection epoch 变化会释放旧订阅并在新 epoch 下重新 attach。quick switch、disable/re-enable 或 width change 后，迟到的 list/attach 结果不能重新填充已清空的 root。
- UI teardown 不调用 `pty.close`；本 fixture 没有需要关闭的真实 disposable PTY。
- terminal operation 缺失或被拒绝只影响 Terminal surface，Chat 与 Settings 仍可导航。

The protocol fixture now reports named scenarios covering Chat/Settings navigation, staged module reuse, exact PTY topics, sequence-gap recovery, selected-session exit transfer, empty-list spawn/attach, UTF-8 and bracketed paste, native Canvas `fillText`, wrong-topic no-op, post-baseline resize, connection-epoch recovery, late-result teardown fencing, and denied `pty.list` without spawn.

protocol fixture 现在报告具名场景，覆盖 Chat/Settings 导航、staged module 复用、exact PTY topics、sequence-gap recovery、selected-session exit 转移、空列表 spawn/attach、UTF-8 与 bracketed paste、Canvas 原生 `fillText`、wrong-topic no-op、baseline 后 resize、connection epoch recovery、迟到结果 teardown fencing，以及拒绝 `pty.list` 不 spawn。

This is protocol-fixture evidence. It uses the real Panel loader, generated terminal JS/WASM and browser DOM, with a test-local host SDK transport. It is not live Gateway evidence.

这是 protocol-fixture evidence：它使用真实 Panel loader、生成的 terminal JS/WASM 和浏览器 DOM，host SDK transport 只在测试边界内提供。它不等价于 live Gateway evidence。

## Production host and wire fixture / Production host 与 wire fixture

Fixture: `interfaces/webchat/tests/panel-terminal-host.test.mjs`.

Command:

```text
cd interfaces/webchat
node tests/panel-terminal-host.test.mjs
```

Result: passed for the production `/terminal` route. The fixture serves the final `.panel-tabs/index.html` and runs the real host WASM/Router/Shell/PanelSdk path. It loads `/aleph_panel.js`, `/aleph_panel_bg.wasm`, `/panel_loader.js`, and the versioned terminal JS/WASM, validates the PTY subscription topics, the `pty.list`/`pty.attach` session id, the `pty.resize` session and positive geometry, and the `pty.input` session/base64 flag/`Aw==` Ctrl-C payload, and observes Canvas output from loopback WebSocket PTY events. The deterministic component fixture separately covers `/` and `/panel/` public bases. The loopback fixture still does not validate authentication or the full Gateway schema.

结果：production `/terminal` route 通过。fixture 提供最终 `.panel-tabs/index.html`，运行真实 host WASM/Router/Shell/PanelSdk 路径；加载 `/aleph_panel.js`、`/aleph_panel_bg.wasm`、`/panel_loader.js` 与版本化 terminal JS/WASM，校验 PTY subscription topic、`pty.list`/`pty.attach` 的 session id、`pty.resize` 的 session 与正数 geometry，以及 `pty.input` 的 session、base64 标记和 `Aw==` Ctrl-C payload，并观察 loopback WebSocket PTY event 驱动的 Canvas 输出。确定性 component fixture 另行覆盖 `/` 与 `/panel/` 两种 public base。loopback fixture 仍不验证认证或完整 Gateway schema。

This proves production host boot and the wire-shaped transport through the real host/module boundary. It does not prove an external Gateway deployment or a real PTY process.

这证明 production host boot 以及真实 host/module 边界上的 wire-shaped transport；它不证明外部 Gateway 部署或真实 PTY 进程。

## Gateway/live result / Gateway/live 结果

The loopback production-host fixture above is not a live Gateway run. The separate read-only Aleph discovery was:

```text
opencode service status
```

Result:

```text
/usr/bin/bash: line 1: opencode: command not found
```

`opencode service status` only proves that the unrelated `opencode` command is unavailable in this shell; it does **not** prove that Aleph Gateway is unavailable. No Aleph Gateway was started, installed, restarted, replaced, or stopped, and no existing PTY/session was touched.

`opencode service status` 只能证明当前 shell 中无关的 `opencode` 命令不可用；它**不能**证明 Aleph Gateway 不存在。本次没有启动、安装、重启、替换或停止 Aleph Gateway，也没有接触已有 PTY/session。

The real Gateway smoke test remains **BLOCKED** because this session did not have an authorized disposable Aleph Gateway/project/session target and credential. Therefore the following remain unverified:

真实 Gateway smoke test 仍为 **BLOCKED**，因为本次没有可授权使用的 disposable Aleph Gateway/project/session target 与 credential。因此以下内容仍未验证：
- real Panel host → SDK → terminal module → Gateway/WS PTY path;
- marker, Unicode, bracketed paste, and resize against a real PTY;
- same-session preservation after route changes;
- transport drop/reconnect against a real connection epoch;
- disposable-session close and cleanup evidence;
- production legacy/modular live parity.

## Same-condition build measurements / 同条件构建测量

Both builds used the current worktree source state, the same `wasm-release` profile, wasm-bindgen, wasm-opt, Tailwind and per-file Brotli quality 11 offline estimate. The legacy build was generated with `just wasm`, then `interfaces/webchat/dist/` was restored with `git restore` so release artifacts were not committed.

两种构建均使用当前 worktree source state、同一个 `wasm-release` profile、wasm-bindgen、wasm-opt、Tailwind，以及每文件 Brotli quality 11 离线估算。legacy 构建使用 `just wasm` 生成，之后执行 `git restore` 恢复 `interfaces/webchat/dist/`，没有把发布产物混入提交。

| Build / 构建 | Host / 首屏 host | Terminal module | Total / 总计 |
|---|---:|---:|---:|
| Legacy raw | 70,536,727 B | included | 70,536,727 B |
| Legacy Brotli estimate | 3,989,588 B | included | 3,989,588 B |
| Modular host raw | 70,448,517 B | — | 70,448,517 B |
| Modular host Brotli estimate | 3,971,243 B | — | 3,971,243 B |
| Modular terminal raw | — | 662,692 B | 662,692 B |
| Modular terminal Brotli estimate | — | 141,695 B | 141,695 B |
| Modular total raw | — | — | 71,111,209 B |
| Modular total Brotli estimate | — | — | 4,112,938 B |

For this report, “first page” is the host asset set: the legacy distribution is entirely host-loaded, while modular defers the versioned terminal module. Browser network timing and WASM instantiate timing were **unavailable** in this static protocol fixture; no RSS or OS memory reclamation numbers were invented.

本报告中的“首屏”指 host asset set：legacy distribution 全部属于 host 首屏，modular 将版本化 terminal module 延迟加载。静态 protocol fixture 没有测量真实 browser network timing 或 WASM instantiate timing，因此记录为 **unavailable**；没有编造 RSS 或 OS memory reclamation 数字。

Tool versions:

```text
rustc 1.99.1 (58cf8f9c3 2026-10-13)
wasm-bindgen 0.2.129
wasm-opt version 133 (version_133)
Bun 1.4.4
Node v24.21.0
Tailwind CSS v4.3.1
```

## Double unlink evidence / 双重去链接证据

1. Dependency graph:

```text
cargo tree -p aleph-panel --no-default-features --features panel-modular --target wasm32-unknown-unknown
```

The filtered modular result contains `aleph-panel` and `shared-ui-logic`, with no `aleph-terminal-ui` or `panel-tab-terminal` dependency.

modular 过滤结果包含 `aleph-panel` 与 `shared-ui-logic`，没有 `aleph-terminal-ui` 或 `panel-tab-terminal` 依赖。

2. Artifact/source inspection:

- `FONT_PROBE_SENTINEL = "1px 'aleph-terminal-font-probe-sentinel'"` is defined by `interfaces/webchat/tabs/terminal-ui/src/render.rs` and the byte is present only in `.panel-tabs/tabs/terminal/ff172f356de7e061/terminal_bg.wasm`; it is absent from `.panel-tabs/aleph_panel_bg.wasm`.
- `data-terminal-view` from the terminal renderer is present only in the terminal WASM artifact and absent from the modular host WASM artifact.
- Shared protocol strings such as `pty.screen` may appear in both artifacts and are not used as the unlink proof.

2. artifact/source 检查：

- `FONT_PROBE_SENTINEL = "1px 'aleph-terminal-font-probe-sentinel'"` 定义于 `interfaces/webchat/tabs/terminal-ui/src/render.rs`，对应字节只出现在 `.panel-tabs/tabs/terminal/ff172f356de7e061/terminal_bg.wasm`，不出现在 `.panel-tabs/aleph_panel_bg.wasm`。
- terminal renderer 的 `data-terminal-view` 只出现在 terminal WASM artifact，不出现在 modular host WASM artifact。
- `pty.screen` 这类 shared protocol string 可能同时出现在两个 artifact，因此没有被用作去链接证据。

Other Tabs remain in the host. This evidence proves Terminal Tab extraction only; it does not claim that the whole Panel is lightweight.

其他 Tabs 仍然在 host 中。本证据只证明 Terminal Tab 的抽离，不宣称整个 Panel 已轻量化。

## Verification / 验证

The Panel build and validation cargo commands used in this workflow were each preceded by `python qa/browser_live/check_memory.py`; the repository's unrelated cargo recipes are outside this acceptance claim. Every gate used the repository policy of requiring at least 4 GiB available memory and returned `ALLOW`.

本次 Panel build 与 validation 使用的 cargo 命令均先执行 `python qa/browser_live/check_memory.py`；仓库中与本验收无关的其他 cargo recipe 不在此声明范围内。每次 gate 都按仓库策略要求至少 4 GiB 可用内存，并返回 `ALLOW`。

Passed:

- `cargo fmt --all -- --check`.
- `cargo test --lib` in `interfaces/webchat/tabs/terminal`: 5 passed.
- `cargo test --lib` in `interfaces/webchat/tabs/terminal-ui`: 44 passed; existing dead-code warnings for non-host render helpers remain.
- `cargo check --lib --target wasm32-unknown-unknown` and `cargo clippy --lib --target wasm32-unknown-unknown -- -D warnings` in `panel-tab-terminal`.
- Panel default/no-feature `cargo check` and `cargo test --lib`: 1,535 passed.
- Panel modular `cargo check`, `cargo clippy --lib --no-default-features --features panel-modular --target wasm32-unknown-unknown -- -D warnings`, and `cargo test --lib --no-default-features --features panel-modular`: 1,573 passed.
- `node tests/panel-terminal-protocol.test.mjs`: named component scenarios passed for `/` and `/panel/`.
- `node tests/panel-terminal-host.test.mjs`: production host/wire fixture passed for the production `/terminal` root route; the component fixture covers `/` and `/panel/` public bases.
- `node tests/panel-build-memory-gate.test.mjs`: isolated rejected memory gate passed; the build exited 1 and the stub `cargo` was not launched.
- `node --test tests/panel-build.test.mjs`: 5 passed.
- `node --test tests/panel_loader.test.mjs`: 8 passed, 1 skipped because `ALEPH_PANEL_BRIDGE_GLUE` was unavailable.
- `bun run check`: unavailable; the repository root has no `check` script and the command returned `error: Script not found "check"`.

## Rollback / 回退

To restore the default legacy distribution after a measurement:

```text
git restore interfaces/webchat/dist
```

The default legacy path remains unchanged and no default release flip, merge, or push was performed by this validation work.

## Remaining limits / 剩余限制

- Live Gateway parity is **BLOCKED**, not passed.
- The production-host/wire fixture uses a loopback Gateway-shaped fixture; it does not prove a real Gateway deployment or PTY process.
- Browser load and WASM instantiate timing are unavailable from the deterministic fixtures.
- Other Panel Tabs have not been migrated or removed from the host.
