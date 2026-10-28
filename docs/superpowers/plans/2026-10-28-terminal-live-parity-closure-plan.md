# Terminal Live Parity Closure Plan / Terminal 真实 Gateway Parity 收口计划

日期：2026-10-28
基线：`main` 已合并 `ec2b174b8 feat(panel): generalize host projection boundary`
相关设计：`docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`
相关交付：`docs/superpowers/plans/2026-10-27-panel-shell-projection-delivery.md`
相关验证：`docs/reports/panel-terminal-tab-validation.md`

## Intent / 意图

本阶段只闭合 Terminal 首个 Tab 的 production host 到真实 Aleph Gateway/PTY 的 live parity。它验证已经通过的 Panel Shell、versioned Terminal module、PanelSdk 和 loopback wire contract 在一个获得明确授权的 disposable Gateway/project/session 上是否真实可用。

当前 loopback host/wire fixture 通过，但 live parity 仍为 `BLOCKED`。缺少授权的 disposable target 时，正确结果是保留 BLOCKED 并报告缺失前置条件；不得把 loopback 结果升级成 live evidence。

## Goals / 目标

- 在授权的 disposable Gateway 上证明 production host 能连接真实 Gateway WebSocket。
- 证明 Terminal route 能通过真实 `pty.list`、`pty.spawn`、`pty.attach`、`pty.input`、`pty.resize` 和事件订阅完成最小闭环。
- 验证 screen/exit 事件、UTF-8 输入、resize 后 geometry、连接 epoch replacement 和最小 reconnect/recovery 行为。
- 只清理本次测试创建的 project/session/PTY/连接资源，并保留可复核的分层 verdict。
- 如果真实环境暴露代码缺陷，先写最小 RED 测试，再做局部修复；没有 live evidence 时不为推测性风险修改生产代码。

## Non-goals / 非目标

- 不迁移第二个 built-in Tab。
- 不切换或删除 `panel-modular` 默认 release，不删除 legacy rollback。
- 不实现 community install/trust/update/uninstall lifecycle。
- 不修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic 或 Gateway authority。
- 不启动、停止、替换用户 Gateway，不关闭不属于本次测试的 PTY/session。
- 不把 loopback、component fixture 或静态 asset build 结果写入 live verdict。
- 不顺便实施 Gateway Bound Stage C 的协议退休或 compatibility closure；那是独立计划。

## Hard gate / 前置闸门

执行者必须先获得以下全部条件：

1. 明确授权的 disposable Aleph Gateway endpoint。
2. 与该 Gateway 配套的临时 credential，以及 project/session 的创建和清理权限。
3. 明确的目标范围和 cleanup owner。
4. 可在当前机器运行的真实 Gateway/browser QA 路径。

credential、token、session id 和 PTY id 只能通过当前进程环境或受控 secret channel 传递，不得写入仓库、文档、报告、trace、截图、静态资产或命令历史。

任一条件缺失时，执行 read-only preflight，记录 `live_gateway: BLOCKED` 及缺失项，停止在 live gate；不得伪造 target、使用用户 Gateway，或用 loopback 替代。

## Approach / 实施路径

沿用已完成的三层证据模型：component fixture → production host/loopback wire fixture → real Gateway/live fixture。只增加第三层 live fixture 和必要的局部修复，保持前两层作为回归基线。

live fixture 必须使用唯一 test run nonce，并按以下顺序验证：

1. 建立真实 Gateway 连接，确认 health/auth/project/session 范围正确。
2. 从 `/terminal` 加载真实 host 和 versioned Terminal assets。
3. 调用真实 `pty.list`，只识别本次测试创建的 PTY。
4. 创建一个带唯一 marker 的 disposable PTY，attach 并等待 screen coverage。
5. 发送 UTF-8 输入和 resize，验证 screen/geometry 到达；验证错误 topic 不会改变当前画布。
6. 触发或等待本次 PTY 的 exit，验证 selected-session fallback 不会关闭其他 session。
7. 在只影响本次测试连接的情况下验证 epoch replacement/reconnect recovery；重新 attach 只使用本次 test-owned PTY。
8. 关闭本次创建的 PTY/session/project 资源，验证 cleanup 到达；浏览器和临时连接随后退出。

## Task sequence / 任务顺序

### 1. Preflight

读取当前两级 `AGENTS.md`、`docs/reference/DEVELOPMENT.md`、`docs/reference/PROCESS_MANAGEMENT.md`、`docs/reference/SECURITY.md`、`qa/README.md`、`docs/reference/TERMINAL_RUNTIME.md` 和本计划。确认 `main` 基线、worktree clean、没有真实 Gateway 进程需要接管。

### 2. Disposable target gate

验证 operator 提供的 target、credential、scope 和 cleanup 权限。先执行只读 health/status 检查；任何 target ambiguity、权限不足或 cleanup 不可证明都停在 BLOCKED。

### 3. Live acceptance

用现有 production host 和真实 Gateway 执行 live fixture。fixture 必须将 component、loopback wire、live 三层结果分别记录，并在失败时保留错误类别、route、RPC method 和 cleanup 状态，不记录 secret 或完整 credential-bearing URL。

### 4. Evidence-driven repair

只有真实 live fixture 给出可重复的 product failure 时才修改代码。修复必须遵循：先写最小失败测试，运行失败，做最小实现，重新运行 affected Rust/Node checks，再重跑 live fixture。不得因为 live target 不可用而做预防性重构。

### 5. Cleanup and report

无论 live fixture 成功还是失败，都执行本次资源的 cleanup，并验证 cleanup 结果。报告必须明确：`component`、`production_host_loopback`、`live_gateway` 三个 verdict；target 是否 disposable；创建了哪些 test-owned 资源；哪些资源已清理；是否仍有 BLOCKED 条件。

## Acceptance / 验收标准

- `component`: 现有 Terminal component fixture 继续通过。
- `production_host_loopback`: 现有 production host 和 protocol fixture 继续通过，且 verdict 明确为 loopback。
- `live_gateway`: 只有真实 Gateway health/auth、真实 WebSocket、真实 PTY screen/input/resize/exit/recovery 和 cleanup 全部有证据时才为 PASS。
- 任一 live 前置条件缺失、cleanup 无法证明或使用了非 test-owned 资源时，`live_gateway` 必须为 BLOCKED/FAIL，并说明原因；不能写 PASS。
- Rust/Node 改动通过项目要求的 memory gate、package-local checks、相关 fixture 和 `git diff --check`。
- 新 worktree clean，提交使用 conventional commit；除非另行授权，不 merge、push 或切换默认 release。

## Deliverables / 交付物

- 如无代码缺陷：一份 live parity validation report，更新必要的 `TERMINAL_RUNTIME` / `FEATURE_LOCATOR` 状态，并保留 loopback 与 live 的分层结论。
- 如有代码缺陷：最小代码 diff、回归测试、live fixture 结果、cleanup 记录和更新后的 validation report。
- 真实 Gateway 未授权或不可用时：一份明确的 BLOCKED report，列出缺失的 target/credential/scope/cleanup 条件，不创建伪造 fixture。
