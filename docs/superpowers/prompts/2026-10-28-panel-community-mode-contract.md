# Prompt: Panel 社区插件自由模式第一阶段

你在 `D:/Workspace/Aleph` 工作。目标是执行：

`docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`

最终产品方向是：Panel UI 模块化，并允许社区插件以自由模式接入；本次只实现可审计的 capability declaration、snapshot source 和 Panel projection 契约，不实现插件安装或动态模块加载。

## 开始前

1. 创建独立 worktree/branch：`panel-community-contract`，不要直接修改共享 `main`。
2. 读取适用的 `AGENTS.md` 和上级规则，以及：
   - `docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`
   - `docs/reference/REDLINES.md`
   - `docs/reference/ARCHITECTURE.md`
   - `docs/reference/FEATURE_LOCATOR.md`
   - `docs/reference/TERMINAL_RUNTIME.md`
   - `interfaces/webchat/src/app.rs`
   - `interfaces/webchat/src/panel/{projection,registry,loader,shell,sdk}.rs`
   - 现有 Panel Node fixtures 和相关 ExtensionManager / Capability 类型。
3. 先运行只读 preflight：`git status --short --branch`、`git log -1 --oneline`、`git worktree list`，确认当前基线是 `c2d8dbf74` 或其后继。
4. 使用 `superpowers:executing-plans` 执行计划；测试先写最小 RED，再实现最小局部修复。

## Hard constraints

- Core 不直接调用平台/UI API；遵守 R1/R2/R4/R8/R9/R10。
- 不修改 `src/harness/`、Capability/ZahirFacade runtime mount、PTY server business logic、Gateway Bound Stage C 或默认 release。
- 不实现插件安装、更新、卸载、签名服务、marketplace、动态 asset build、JS/WASM load 或真实 Gateway smoke。
- 不接 production ExtensionManager，除非现有类型已经提供无额外 lifecycle 语义的只读 snapshot；默认使用 in-process fixture source。
- 不创建第二份浏览器 descriptor、registry、loader 或权限模型。现有 `PanelTabDescriptor` 是浏览器侧唯一 projection shape。
- 社区声明中的 entry 必须是受约束的 versioned relative path；禁止外部 URL、路径穿越、隐式 public base 和不受支持的 permission/trust。
- 任何拒绝必须 fail-closed，并返回稳定的非 secret reason code；不能返回成功的 no-op。
- 每个 cargo 子进程前，从 worktree root 运行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从所属 package 目录串行执行。
- 不运行真实 Gateway。不要产生、保存或输出 credential、token、session id、PTY id。
- 不 merge、push 或切换默认 release，除非获得新的明确授权。

## Required implementation shape

优先实现以下边界，若现有 schema 证明需要不同命名，必须在报告中说明原因：

1. `PanelUiCapabilityDeclaration`：包裹现有 `PanelTabDescriptor`，并携带来源、trust、plugin identity/version、asset integrity、permissions 和 source revision 所需事实。
2. `PanelUiCapabilitySnapshot`：source 的唯一输出，带稳定 revision 和声明集合；不得暴露可变 registry。
3. 一个 source trait/adapter：先提供 built-in Terminal source 和 fixture source。built-in 输出必须与现有 `PanelProjection::builtins(...)` 完全一致。
4. `PanelProjection` 的单一验证入口：验证 identity、origin、trust、permissions、integrity、relative entry、public base、revision、duplicate/collision 和 deterministic ordering，再输出现有 `PanelTabDescriptor` projection。
5. snapshot replacement/late-result fencing：旧 revision 不能覆盖新 snapshot，不能重新激活已拒绝或已卸载的 descriptor。

不要为了实现这些边界新增 app 层 registry，也不要把 fixture source 当成真实社区安装流程。

## Required tests

至少覆盖：

- 一个合法 community declaration 可以被稳定投影；
- duplicate ID；
- community 与 built-in ID collision；
- 外部 URL、路径穿越、错误 public base；
- 缺失或错误 asset integrity；
- 未知 trust 和未声明 permission；
- revision replacement 后旧 late result 被拒绝；
- deterministic ordering；
- 内置 Terminal descriptor、route、entry、PanelShell/PanelSdk/PanelLoader 行为与 baseline 一致；
- 现有 panel build/artifact/protocol/host fixtures 不回归。

如果测试揭示真实 production defect，只修改 owner 所在的最小边界，并记录 RED、修复、重跑结果。没有证据不要做预防性重构。

## Verification and handoff

根据实际修改运行适用检查：

- memory gate；
- 受影响 package 的 `cargo check` / `cargo test` / `cargo clippy`；
- 受影响 Panel Node fixtures；
- `git diff --check`；
- 必要的独立只读 review。

报告必须明确：

- 哪些 declaration/snapshot/projection 事实已验证；
- built-in Terminal 是否保持兼容；
- 哪些社区插件能力仍未实现；
- 没有运行真实 Gateway；
- 没有 dynamic JS/WASM load、install/update/uninstall 或 production ZahirFacade mount。

最后确认 worktree clean，创建 conventional commit，例如：

`feat(panel): add community capability projection contract`

不要自行 merge 或 push；把 commit、验证结果、未完成范围和下一阶段建议返回给主 agent。
