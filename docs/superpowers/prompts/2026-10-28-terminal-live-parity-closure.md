# Restart Prompt: Terminal Live Parity Closure / 重启 Prompt：Terminal 真实 Gateway Parity 收口

你在 `D:/Workspace/Aleph` 的 `main` 上继续工作。`panel-shell` 已合并，当前起点是已提交的通用 Panel Shell projection/registry/route/loader boundary。请先执行 `git status --short --branch` 和 `git log -1 --oneline`，确认工作区干净，再创建独立 worktree/branch；不要直接在共享 `main` 工作区实现。

## Objective / 目标

完成 `docs/superpowers/plans/2026-10-28-terminal-live-parity-closure-plan.md`：验证 Terminal production host、versioned Terminal module、PanelSdk 与真实 Aleph Gateway/PTY 的 live parity，并保持 component、loopback wire、live Gateway 三层 evidence 分离。

本任务只处理 Terminal live parity。不要迁移第二个 Tab，不要实施 Gateway Bound Stage C，不要切换默认 release，不要修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic、community lifecycle 或其他 Tab 的业务逻辑。

## Mandatory gate / 强制闸门

在任何 live 操作前，必须得到明确授权的 disposable Gateway/project/session/credential，以及本次测试资源的 cleanup 权限。credential、token、session id、PTY id 只能留在进程环境或受控 secret channel，不得写入文件、报告、trace、截图、静态资产或命令历史。

如果当前没有授权 disposable target，完成只读 preflight 后立即停止并报告：

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

不要连接用户 Gateway，不要停止、替换或清理不属于本次测试的 Gateway、PTY、session 或 project，也不要把 loopback fixture 当作 live evidence。

## Required reading / 必读

- `AGENTS.md` 和上级适用的 `AGENTS.md`
- `docs/superpowers/plans/2026-10-28-terminal-live-parity-closure-plan.md`
- `docs/superpowers/plans/2026-10-27-panel-shell-projection-plan.md`
- `docs/superpowers/plans/2026-10-27-panel-shell-projection-delivery.md`
- `docs/reports/panel-terminal-tab-validation.md`
- `docs/reference/TERMINAL_RUNTIME.md`
- `docs/reference/DEVELOPMENT.md`
- `docs/reference/PROCESS_MANAGEMENT.md`
- `docs/reference/SECURITY.md`
- `qa/README.md`
- `interfaces/webchat/src/panel/{projection,registry,loader,shell}.rs`
- `interfaces/webchat/src/app.rs`
- `interfaces/webchat/tests/panel-terminal-host.test.mjs`
- `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`
- `interfaces/webchat/tests/fixtures/panel-gateway.mjs`

## Execution rules / 执行规则

1. 使用 `panel-live-parity` 等不超过三个词的 branch/worktree 名；不要 merge 或 push，除非当前会话收到新的明确授权。
2. 每个 cargo 子进程前，从 worktree root 执行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从所属 package 目录运行并串行执行。
3. 先复用现有 component/loopback fixture 作为回归基线，再连接真实 target。
4. 为本次测试生成唯一 nonce，只创建和记录 test-owned project/session/PTY。
5. live acceptance 至少证明：真实 Gateway health/auth、WebSocket 连接、`/terminal` host 资产加载、`pty.list`、`pty.spawn`、`pty.attach`、真实 screen event、UTF-8 input、resize/geometry、PTY exit/fallback、test-owned reconnect/epoch recovery，以及资源 cleanup。
6. live fixture 输出必须分开报告 `component`、`production_host_loopback`、`live_gateway` verdict；禁止将 loopback 或静态 build 结果写成 live PASS。
7. 只有真实 live fixture 暴露可重复的产品缺陷时才改代码；先写最小 RED 测试，再做局部实现，最后重跑受影响的 Rust/Node/live checks。
8. teardown 只能释放 DOM、subscription、RAF、observer、polling、module handles 和本次测试拥有的 Gateway 资源；route leave/disable 不能调用 `pty.close` 去关闭不属于本次测试的 session。
9. 成功或失败都必须执行 cleanup，并在 report 中记录 cleanup 是否完成；如果 cleanup 无法证明，live verdict 不能是 PASS。

## Completion / 完成标准

- 有真实 target 时：live acceptance 全部通过，报告包含可复核的非敏感证据，相关 docs 更新，worktree clean，并创建 conventional commit。
- 没有真实 target 时：不写生产代码、不伪造 fixture，留下明确 BLOCKED report，列出需要 operator 提供的 target、credential、scope 和 cleanup 条件。
- 最终报告必须明确未做的内容：第二个 Tab、默认 release flip、Gateway Bound Stage C、community lifecycle、全量 mobile/accessibility/release hardening。
