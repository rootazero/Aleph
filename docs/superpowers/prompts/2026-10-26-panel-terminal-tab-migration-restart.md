# Restart Prompt — Panel Terminal Tab Migration

> 用法：新会话从当前主仓库打开，粘贴下方代码块中的提示词。本文件本身不启动实施。 / Paste the prompt below into the next session; this file does not execute the plan.

```text
实施 Aleph 下一阶段：真实 Terminal Tab WASM 拆包与导航接入。

先完整读取：
1. AGENTS.md（以及适用的上级项目约束）
2. docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md
3. docs/superpowers/plans/2026-10-26-panel-terminal-tab-migration-plan.md
4. docs/superpowers/prompts/2026-10-26-panel-terminal-tab-migration-restart.md（本文件）

我已选择 Subagent-driven；实现必须在独立 Git worktree。请按上述计划逐任务执行、独立审查和主 Agent 验收，推进到完成或真实阻塞，不要重新做 brainstorming 或上一阶段六个任务。

基线与分支非常重要：
- 上阶段已经完成在本地 panel-tab-spike 分支，已验收 HEAD：
  c5146f30c64d19a70fce20c799b88e04fd0b5567
- 它的 worktree 是仓库内 .worktrees/panel-tab-spike，尚未合并/推送。
- 写本交接时 main 是 86ffeeb74，不包含 spike 产品代码；不要从 main 直接开始新实现。
- 先检查 git root/status/worktrees/提交；然后从上述已验收 spike 提交创建
  panel-terminal-tab 分支和 .worktrees/panel-terminal-tab worktree。
- 若主工作区的本计划与本提示词尚未提交，只复制这两份文档到新 worktree
  并随首个相关提交保存；不要复制别的改动，不要覆盖已有 worktree。
- 本机已核实不存在 origin/dev；不要为满足泛用默认分支规则去虚构远端。
- 当前先不要合并、推送或切换默认发布构建。

本阶段交付：
1. 窄 SDK 补齐精确 PTY topic 订阅、连接 epoch 和有界 UI 事件队列。
2. 将现有真实 Canvas 终端提取成一份 terminal-ui Rust 实现。
3. 创建独立 panel-tab-terminal WASM 入口，用同一本地 UI 实现渲染左右面板。
4. 模块化构建不链接 terminal-ui；Chat/Settings 等原入口保持可用。
5. 使用实际发布资产链、相对 public base 和同构建 manifest；补齐 CSS 扫描。
6. 完整链路 fixture + 真实 Gateway 验证，记录同条件体积与加载测量。

上一阶段只证明 pilot artifact 的懒加载/双根清理，未证明真实 TerminalView 迁移、
真实 Shell/Gateway/WS 端到端或总资源下降。不要把 skipped tests 当作通过。
详细证据已在设计文档的 Spike results；忽略目录中的日志不是唯一交付依据。

重要边界：
- 使用 superpowers:subagent-driven-development 与 using-git-worktrees。
- 一次一个子代理写当前 worktree，依赖任务顺序推进；审查必须只读。
- 子代理只使用环境实际支持的模型/工具；所有 gstack 审查经 Pi，不能跑模型 CLI。
- 子代理失败先读输出/活动日志，保留已完成代码；同一问题无进展最多三次有效尝试。
- SDK 不跨 WASM 传 DashboardState/Leptos Context/Signal/Rust 对象或裸 WebSocket。
- 卸载/禁用 UI 不关闭 PTY。pty.close 仅来自用户显式关会话或已授权测试清理。
- 保留默认 legacy 构建，新增 opt-in panel-modular；不实现社区安装链、
  不迁移所有 Tab、不挂 ZahirFacade、不增加 Harness 智慧或 PTY 服务器业务逻辑。
- 每次 cargo 前跑 qa/browser_live/check_memory.py；包目录跑测试，编译串行。
- 真实 Gateway 不可用时如实记录 BLOCKED，不能用 fixture 冒充生产验证，
  也不能未经授权重启/替换用户的常驻服务或干扰其现有 PTY。

先简要重述本阶段验收目标和真实基线，完成 preflight 后开始 Task 1。
最终用中文报告修改、证据、分支/worktree、实际测试和未做/阻塞项。
```

该提示词沿用用户的执行方式与 worktree 约束；下一阶段实现决策及验收细节以关联计划为准。 / Execution method and worktree isolation are preserved; the linked plan owns the implementation contract and acceptance criteria.
