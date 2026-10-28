# Restart Prompt: Terminal Build E2E / 重启 Prompt：Terminal 真实构建 E2E

你在 `D:/Workspace/Aleph` 的 `main` 上开始下一阶段工作。上一阶段的 `panel-live-parity` 分支已经快进合入 `main`，当前集成点包含：

```text
f48991a5b docs(panel): record terminal live parity block
```

请先执行 `git status --short --branch`、`git log -1 --oneline` 和 `git worktree list`，确认共享 `main` 干净；随后创建独立 worktree/branch，例如 `terminal-build-e2e`。实现、构建和测试都写入新 worktree，不要直接在共享 `main` 修改。不要 merge、push 或切换默认 release，除非当前会话获得新的明确授权。

## Objective / 目标

完成 `docs/superpowers/plans/2026-10-28-terminal-build-e2e-plan.md`。重点是进行真实的 build E2E：从当前源码重新生成 Panel modular candidate，再由本次生成的最终 staged HTML 启动真实 production host WASM、PanelShell、PanelLoader、PanelSdk、versioned Terminal JS/WASM，并通过 loopback HTTP/WebSocket Gateway fixture 完成浏览器验收。

现有 `docs/reports/panel-terminal-live-parity-closure.md` 的事实必须保留：

```text
component: PASS
production_host_loopback: PASS
live_gateway: BLOCKED
```

新的 build E2E 必须证明“当前源码构建产物被真实 host 加载”，不能把旧 `.panel-tabs` 静态资产、loopback wire 或 build PASS 写成真实 Gateway parity。

## Scope / 范围

本任务只处理 Terminal build E2E、production host loopback evidence 和真实 Gateway 的条件式 smoke。不要迁移第二个 Tab，不要实施 Gateway Bound Stage C，不要切换默认 release，不要修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic、community lifecycle 或其他 Tab 的业务逻辑。

## Mandatory live gate / 真实 Gateway 闸门

在任何真实 Gateway 操作前，必须取得明确授权的 disposable Gateway/project/session/credential，以及本次测试资源的 cleanup 权限。credential、token、session id、PTY id 只能留在当前进程环境或受控 secret channel，不得写入文件、报告、trace、截图、静态资产或命令历史。

如果没有完整授权 target，执行只读 target preflight，然后停止 live 操作并写入：

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

不要连接用户 Gateway，不要停止、替换或清理不属于本次测试的 Gateway、project、session 或 PTY，也不要把 loopback fixture 当作 live evidence。

## Required reading / 必读

- 适用的 `AGENTS.md` 和上级 `AGENTS.md`
- `docs/superpowers/plans/2026-10-28-terminal-build-e2e-plan.md`
- `docs/superpowers/plans/2026-10-28-terminal-live-parity-closure-plan.md`
- `docs/superpowers/prompts/2026-10-28-terminal-live-parity-closure.md`
- `docs/reports/panel-terminal-live-parity-closure.md`
- `docs/reports/panel-terminal-tab-validation.md`
- `docs/reference/TERMINAL_RUNTIME.md`
- `docs/reference/DEVELOPMENT.md`
- `docs/reference/PROCESS_MANAGEMENT.md`
- `docs/reference/SECURITY.md`
- `qa/README.md`
- `interfaces/webchat/scripts/build-panel-tabs.mjs`
- `interfaces/webchat/package.json`
- `interfaces/webchat/src/app.rs`
- `interfaces/webchat/src/panel/{projection,registry,loader,shell,sdk}.rs`
- `interfaces/webchat/tests/panel-terminal-host.test.mjs`
- `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`
- `interfaces/webchat/tests/fixtures/panel-gateway.mjs`

先用自己的话复述目标、当前三层证据边界和本轮 build-first approach，再开始 preflight；不要重新扩展为全 Panel 设计讨论。

## Execution rules / 执行规则

1. 使用不超过三个词的 branch/worktree 名，例如 `terminal-build-e2e`。
2. 先运行现有 component fixture，保留其独立 verdict；再从源码构建并运行 production-host loopback E2E。
3. 每个 cargo 子进程前，必须从 worktree root 执行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从所属 package 目录运行并串行执行。构建脚本内部的 `cargo build`、`cargo tree`、版本探测或新增 cargo 子进程同样适用。
4. 先检查提交 hook 是否间接启动 cargo；不能把未按 memory gate/cwd 规则执行的 hook 结果描述成验证通过。
5. 只使用本次构建产生的 output root。不要把共享 checkout 的 `.panel-tabs` 拷贝进 worktree 来伪造 build E2E。
6. 不输出任何 credential 值、完整授权 URL、session/PTY id 或 secret-bearing trace。
7. 只有真实 E2E 暴露可重复产品缺陷时才修改代码；先写最小 RED，再做局部实现，重新构建 staged output，重跑受影响 Rust/Node/Playwright checks。
8. teardown 只能释放 DOM、subscription、RAF、observer、polling、module handles 和本次测试创建的 Gateway 资源；route leave/disable 不得调用 `pty.close` 关闭外部 session。
9. 成功或失败都要执行 cleanup；无法证明 cleanup 时 `live_gateway` 不能为 PASS。

## Required implementation / 必须完成

### 1. Build from source

从 `interfaces/webchat` 运行现有 `panel:tabs:build` 或等价命令，使用隔离 output root 和 clean build。构建结果必须由当前 HEAD 产生，并记录非敏感 build ledger：revision、profile、build id、manifest entry、source input hash、工具版本、host/terminal descriptor identity、WASM/JS/HTML/CSS 资产列表和每个 cargo gate 结果。

确认：

- `build.json.revision` 等于被测 HEAD；
- `manifest.entry` 指向本次 build id 的 versioned Terminal module；
- descriptor 的 plugin/tab identity 与 entry 一致；
- `index.html` 中没有未处理的 Trunk asset；
- host 和 Terminal 的 wasm-bindgen glue/WASM、loader、CSS 和 versioned module 均存在；
- stale build id、缺失 asset、错误 descriptor 或错误 public base 会明确失败；
- 临时 target 不被统计为 deployed asset 或首屏传输。

### 2. Real build production-host fixture

扩展现有 `panel-terminal-host.test.mjs` 或创建 build-aware runner，使测试接收本次构建的 output root，不固定消费历史 `.panel-tabs`。loopback server 原样服务本次构建的 HTML、JS、WASM、loader、CSS 和 versioned Terminal module，只在 HTTP/WS 边界模拟 Gateway。

Playwright 必须从最终 staged `index.html` 开始，证明：

- 真实 `aleph_panel.js`/host WASM 被请求并初始化；
- Chat/Settings 页面不会提前请求 Terminal module；
- 真实生产导航进入 Terminal，真实 PanelShell/PanelSdk/PanelLoader 挂载 Terminal；
- 浏览器请求的 Terminal JS/WASM 使用本次 build id；
- production Gateway WebSocket 完成 connect、topic subscription、`pty.list`、`pty.attach`，必要时 `pty.spawn`；
- 实际 screen event 产生可见 marker；UTF-8 input 到达正确 RPC；resize 在 baseline 后产生新 geometry；selected PTY exit/fallback 可见；
- reconnect/epoch replacement 使用新 socket，旧 list/attach/subscribe 结果不能覆盖新状态或复活已卸载 DOM；
- `/`、`/panel/`、direct `/terminal`、`/panel/terminal` 的正向路径有正数结果；
- pageerror、failed request、实际 HTTP asset path、RPC method 和 cleanup 状态被记录，不记录 secret。

禁止使用手写替代 HTML、替代生产 Shell/SDK、`page.evaluate` 注入导航、替代 DashboardState 或直接调用 module API 来宣称 production-host E2E。

### 3. Component and failure regression

先重跑现有 `panel-terminal-protocol.test.mjs`，保持 component evidence 独立。按实际缺口补充效果级断言：真实 selected session exit、spawn/attach、baseline 后 resize、原生 Canvas marker、exact topic、late result fencing、missing/incompatible module failure isolation、`pty.list` failure 和 route teardown 不关闭非 test-owned PTY。

只修测试证明的 owner/epoch/loader/SDK/Terminal lifecycle 缺陷；不做预防性重构。

### 4. Conditional live smoke

仅在 operator 明确提供完整 disposable target 后运行真实 Gateway acceptance。用唯一 test-owned project/session/PTY 证明真实 health/auth、WebSocket、`/terminal` asset load、`pty.list`、`pty.spawn`、`pty.attach`、screen、UTF-8 input、resize/geometry、PTY exit/fallback、test-owned reconnect/epoch recovery 和 cleanup。

没有 target 时继续完成 build E2E 和 loopback report，然后将 live verdict 保持为 BLOCKED；不得因 build E2E 通过而升级 live verdict。

### 5. Report and docs

更新双语 validation report，明确写出：

```text
component: PASS|FAIL|BLOCKED
production_host_loopback: PASS|FAIL|BLOCKED
live_gateway: PASS|FAIL|BLOCKED
```

报告必须包含 build revision/build id、命令与 cwd、memory gate、真实浏览器 asset 请求、host/module 初始化、RPC/事件证据、cleanup 结果和未完成范围。更新 `TERMINAL_RUNTIME`/`FEATURE_LOCATOR` 只写已验证事实，保留 live BLOCKED 条件。

## Checks / 验证

按实际改动运行适用检查，并记录精确结果：

- `python qa/browser_live/check_memory.py`：每个 cargo 前，从 worktree root 执行；
- component Node fixture；
- build artifact/manifest/descriptor checks；
- production-host Playwright/Node fixture，使用本次新构建 output；
- 受影响 package 的 Rust tests/check/clippy；
- `git diff --check`；
- 适用的独立只读 review。

不要声称未执行的检查通过。若 root 没有 canonical `bun run check`，记录 unavailable 并使用实际 package checks。

## Completion / 完成标准

- 本次源码重新构建的 output 被最终 staged HTML 和浏览器真实加载；不是旧 ignored asset。
- build id、revision、manifest、descriptor、HTTP 请求和 module load 一致。
- production host loopback E2E 经过真实 host WASM、Shell、SDK、Terminal WASM 和 WebSocket fixture。
- 三层 evidence 独立报告，live target 缺失时为 BLOCKED。
- 所有 test-owned 资源 cleanup 有证据；无法证明时 live 不能 PASS。
- 没有真实缺陷就不改生产代码；有缺陷则有 RED、最小修复、重建和回归证据。
- worktree clean，使用 conventional commit；不 merge、不 push、不切换默认 release。

明确未做：第二个 Tab、Gateway Bound Stage C、默认 release flip、community lifecycle、全量 mobile/accessibility/release hardening、完整发布回滚矩阵和非 Terminal 业务逻辑。
