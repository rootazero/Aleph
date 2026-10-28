# Terminal Build E2E / Terminal 真实构建 E2E 计划

日期：2026-10-28  
基线：`main` 已快进合入 `f48991a5b docs(panel): record terminal live parity block`  
上一阶段报告：`docs/reports/panel-terminal-live-parity-closure.md`  
上一阶段结论：`component: PASS`、`production_host_loopback: PASS`、`live_gateway: BLOCKED`

## Intent / 意图

下一阶段把 Terminal 验收从“消费已有 staged assets 的 host fixture”提升为“从当前源码生成候选构建，再由最终 staged HTML 启动真实生产 host 的 build E2E”。目标是证明同一份源码状态产生的 host WASM、Panel loader、versioned Terminal JS/WASM、descriptor 和 manifest 能在浏览器中经过真实 `PanelShell → PanelSdk → DashboardState → WebSocket → Terminal module` 链路工作。

Build E2E 的成功只证明 `production_host_build_loopback`，不证明真实 Aleph Gateway parity。真实 Gateway 仍必须拥有独立的 `live_gateway` verdict、明确的 disposable target、credential scope、test-owned 资源和 cleanup 证据。

## Goals / 目标

- 在新 worktree 中从源码重新构建 Panel modular candidate，不复用主工作区的旧 `.panel-tabs` 资产作为测试输入。
- 校验 build revision、build id、manifest entry、编译 descriptor、host/terminal WASM 和最终 HTML 的一致性。
- 用构建脚本刚生成的 staged output 启动最终 HTML，证明浏览器确实请求并初始化真实 host WASM 与 versioned Terminal module。
- 通过真实生产 Router、PanelShell、PanelLoader、PanelSdk 和 loopback HTTP/WebSocket Gateway fixture 完成 Terminal 基本交互。
- 保持三层 evidence 分离：`component`、`production_host_loopback`、`live_gateway`。
- 对构建、浏览器、RPC、资源生命周期和 cleanup 记录可复核但不含 secret 的证据。
- 只有真实 E2E 暴露可重复产品缺陷时才写最小 RED 测试并修复。

## Non-goals / 非目标

- 不迁移第二个 built-in Tab。
- 不实施 Gateway Bound Stage C。
- 不切换默认 release，不删除 legacy rollback。
- 不修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic、community lifecycle 或其他 Tab 业务逻辑。
- 不把 build 绿色、loopback WebSocket 或静态 asset 请求写成真实 Gateway PASS。
- 不自动启动、停止、替换用户 Gateway，也不操作非 test-owned project/session/PTY。
- 不把体积估算、offline Brotli 数值或静态文件存在性称为首屏 transfer、浏览器回收或 live evidence。

## Approaches / 实施方案

### A. 继续使用已有 `.panel-tabs` 做浏览器验收

成本最低，但无法证明当前源码经过构建后产生了被测试的 host/module；旧资产可能来自其他 revision，也无法覆盖构建脚本、manifest、descriptor 和生成 HTML 的一致性。保留为回归 fixture 输入，不作为本阶段主路径。

### B. Build-first production-host E2E（推荐）

每次 acceptance 在隔离 worktree 中先执行源码构建，再把刚生成的 output root 交给 loopback server。浏览器打开最终 staged `index.html`，通过生产 host 和真实模块加载 Terminal；测试同时检查构建账本、HTTP 请求、WASM 初始化、RPC 和可见效果。成本高于 A，但能直接证明本阶段要解决的“真实构建 E2E”缺口。

### C. 先连真实 Gateway，再验证构建

真实度最高，但会把构建问题、Gateway 授权问题和 PTY 生命周期问题混在同一失败面中；当前也没有授权 disposable target。真实 Gateway 只作为 B 通过后的独立条件分支。

本阶段选择 **B + 条件式 C**：先完成 build-first loopback evidence，再在 operator 明确提供 disposable target 时运行 live smoke；live 缺失时保留 `BLOCKED`。

## Hard gate / 强制闸门

在任何真实 Gateway 操作前，必须同时具备：

1. 明确授权的 disposable Gateway endpoint 或启动范围。
2. 只用于本次测试的 credential，并通过进程环境或受控 secret channel 注入。
3. test-owned project、session 和允许创建 PTY 的权限。
4. 明确 cleanup owner，且只覆盖本次测试创建的 Gateway/project/session/PTY/连接资源。
5. 可证明的 cleanup 路径。

credential、token、session id、PTY id 不能写入仓库、报告、trace、截图、静态资产或命令历史。任一条件缺失时只做 read-only preflight 与 build/loopback E2E，输出：

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

## Execution rules / 执行规则

- 从干净 `main` 创建不超过三个词的 worktree/branch，例如 `terminal-build-e2e`；实现和测试不写共享 `main`。
- 每个 cargo 子进程前，必须从 worktree root 执行 `python qa/browser_live/check_memory.py` 并接受 exit 0；cargo 从所属 package 目录运行并串行执行。
- 构建脚本内部的 `cargo build`、`cargo tree`、版本探测或新增 cargo 子进程同样遵守 memory gate 和 package cwd 规则。
- 先运行已有 component fixture，再运行 build-first production-host fixture；任何 fixture 绿色都不能覆盖另一层 verdict。
- 每次构建使用唯一 build run nonce 仅用于测试标签；build id 仍由源码/输入派生。nonce、credential 和 live ids 不写入报告。
- 失败时保留失败类别、revision、route、RPC method、asset path 和 cleanup 状态，不保留 credential-bearing URL 或 secret payload。
- 如需修改产品代码，先写最小 RED，确认失败，再局部实现，重建 staged output，重跑受影响的 Rust/Node/Browser checks。
- 提交前检查 hook 是否会间接启动 cargo；不能把未按 gate/cwd 执行的 hook 结果描述成已验证。不得 merge、push 或切换默认 release，除非得到新的明确授权。

## Task sequence / 任务顺序

### 1. Preflight and isolation

读取适用的 `AGENTS.md`、本计划、`docs/superpowers/plans/2026-10-28-terminal-live-parity-closure-plan.md`、`docs/reference/TERMINAL_RUNTIME.md`、`DEVELOPMENT.md`、`PROCESS_MANAGEMENT.md`、`SECURITY.md`、`qa/README.md`、现有 host/protocol fixture 和 live blocked report。确认 `main` clean、HEAD、工具版本、当前没有需要接管的 Gateway。创建 `terminal-build-e2e` worktree。

### 2. Build candidate from source

从 `interfaces/webchat` 执行现有 `panel:tabs:build` 等价构建，使用隔离 output root 和 `--clean`，不读取共享 checkout 的 ignored `.panel-tabs` 作为输入。记录非敏感的：

- HEAD、profile、target、build id、manifest entry；
- host/terminal descriptor identity 与 entry equality；
- `index.html`、`aleph_panel.js`、`aleph_panel_bg.wasm`、`panel_loader.js`、versioned `terminal.js`/`terminal_bg.wasm` 和 CSS 是否存在；
- `build.json`、依赖树、工具版本和 source input hash；
- 每个 cargo gate 的 exit 结果与 package cwd。

构建验证必须证明 output 是本次源码状态新生成的；不能只检查文件存在。构建输出是分发候选证据，不是 live Gateway evidence。

### 3. Build artifact contract checks

增加或复用确定性检查，至少验证：

- `build.json.revision` 等于被测 HEAD；
- `manifest.entry` 指向本次 build id 的 versioned Terminal module；
- 编译 Terminal descriptor 的 `key.plugin_id`、`key.tab_id`、entry 与 manifest 一致；
- host 与 Terminal wasm-bindgen glue、WASM、loader 和 final HTML 的相对路径在 `/` 与 `/panel/` 两种 public base 下可解析；
- stale output、错误 build id、缺失 module、未处理 Trunk asset 会明确失败；
- `.panel-tabs/target` 等临时 build 目录不会被误当成 deployed asset，也不会混入报告的首屏 transfer 统计。

### 4. Real build production-host E2E

扩展现有 `panel-terminal-host.test.mjs` 或新增 build-aware runner，使其接收本次构建的 output root，而不是固定依赖旧 `.panel-tabs`。loopback server 必须原样服务构建生成的 HTML、JS、WASM、loader、CSS 和 versioned module，并只在 HTTP/WS 边界模拟 Gateway。

Playwright 必须：

- 打开最终 staged `index.html`，证明 `aleph_panel.js`/host WASM 被实际请求并初始化；
- 通过真实生产导航进入 Terminal，证明 Chat/Settings 不会提前加载 Terminal module；
- 证明 `panel_loader.js` 加载本次 build id 的 Terminal JS/WASM；
- 通过真实 `PanelShell`、`PanelSdk` 和 loopback WebSocket 完成 connect、`pty.list`、`pty.attach`，必要时 `pty.spawn`；
- 发送 UTF-8 input，验证 `pty.input` payload；改变几何后验证新的 `pty.resize`；注入真实 selected session 的 screen/exit event，验证可见 screen 和 fallback；
- 验证精确 `pty.screen`/`pty.exit` subscriptions、epoch replacement、迟到结果不能复活已卸载 DOM；
- 在 `/`、`/panel/`、direct `/terminal` 和 `/panel/terminal` 覆盖正向路径；
- 记录 page errors、failed requests、实际 asset paths 和 RPC method，但不记录 credential 或完整授权 URL；
- finally 关闭 browser、fixture server、socket、subscription、RAF、observer 和 module handles。

### 5. Component and failure regression

先复用现有 component fixture，确保 build E2E 没有掩盖 component 层缺口。对真实构建 candidate 追加必要的失败隔离：missing module、incompatible `api_major`、`pty.list` failure、旧 socket 迟到 list/attach/subscribe，以及 route leave 不关闭非 test-owned PTY。只有被测试证明的产品问题才修改对应 owner。

### 6. Conditional live Gateway smoke

Build E2E 和 loopback 通过后，再执行独立 target gate。使用唯一 test-owned project/session/PTY，证明真实 health/auth、WebSocket、`/terminal` host asset load、`pty.list`、`pty.spawn`、`pty.attach`、真实 screen、UTF-8 input、resize/geometry、PTY exit/fallback、test-owned reconnect/epoch recovery 和 cleanup。只允许关闭本次创建的 PTY/session/project/Gateway 资源；route leave/disable 不能调用 `pty.close` 关闭外部 session。

无授权 target 时不得连接任何 Gateway，报告 `live_gateway: BLOCKED`；loopback build E2E 仍单独记录为 PASS 或 FAIL。

### 7. Report, docs and review

报告明确列出：

```text
component: PASS|FAIL|BLOCKED
production_host_loopback: PASS|FAIL|BLOCKED
live_gateway: PASS|FAIL|BLOCKED
```

同时记录构建 revision/build id、构建命令与 cwd、memory gate、真实浏览器 asset 请求、host/module 初始化、RPC/事件证据、cleanup 状态和未测项目。更新 `TERMINAL_RUNTIME`/`FEATURE_LOCATOR` 只反映已验证事实；不把 build E2E 写成 live parity。完成适用 package checks、Node/Playwright checks、`git diff --check`，并做一次只读独立 review。

## Acceptance / 验收标准

- 构建从当前源码重新产生候选 output；不依赖旧 ignored assets。
- `build.json`、manifest、descriptor、build id、revision 和实际 HTTP 请求一致。
- 最终 staged HTML 经过真实 host WASM、生产 Shell/SDK、versioned Terminal module 和 loopback WS；root/prefixed bases 有正数通过。
- component、production host loopback、live Gateway 三层 verdict 分离；live 缺 target 时明确 BLOCKED。
- 所有真实创建资源的 cleanup 有证据；cleanup 无法证明时 live 不能 PASS。
- 未发现产品缺陷时不修改生产代码；发现缺陷时有最小 RED、局部修复和重建后回归证据。
- worktree clean，提交使用 conventional commit；不 merge、不 push、不切换默认 release。

## Deliverables / 交付物

- 本次源码构建的非敏感 build ledger 和 manifest/descriptor 校验结果。
- build-first production-host E2E fixture 及其可复现命令。
- 更新后的双语 Terminal validation report。
- 如有真实缺陷：最小修复、回归测试和受影响检查结果。
- 如无 live target：明确 BLOCKED report，列出 operator 必须提供的 target、credential、scope 和 cleanup 条件。

## Explicitly deferred / 明确后置

第二个 Tab、Gateway Bound Stage C、默认 release flip、community lifecycle、全量 mobile/accessibility/release hardening、完整发布回滚矩阵和非 Terminal 的 Panel 业务逻辑均不在本阶段。
