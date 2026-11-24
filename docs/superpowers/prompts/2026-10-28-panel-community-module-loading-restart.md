# Panel community module loading — next-session restart prompt

实施 Panel 社区 UI 的下一阶段：**把已验证 declaration 中的 asset integrity 贯通到现有 PanelLoader，在导入前校验精确资产字节，并证明受控单文件社区 ES module 的加载、挂载与清理语义**。

上一阶段已由 `aab26f257 feat(panel): add community capability projection contract` 完成 declaration / snapshot / source / projection 契约，并合入 `main`。本阶段不是插件安装流程，也不是 production discovery 或默认 release 切换。

## Starting point / 起点

- 当前契约基线为 `aab26f257`；若 `main` 已有后继文档或代码提交，从最新 `main` 创建独立 worktree/branch `panel-community-loader`，并确认 `aab26f257` 是祖先。
- 不直接在共享 `main` 工作区实现，不 force/reset/delete 既有 worktree。
- 使用 `superpowers:executing-plans` 顺序执行：
  `docs/superpowers/plans/2026-10-28-panel-community-module-loading-plan.md`。
- 测试先写最小 RED，确认失败原因后实现最小局部修复；最终独立只读 review、conventional commit、worktree clean。
- 不 merge、不 push，除非收到新的明确授权。

## Read first / 先读

1. 当前 git root 与上级适用的 `AGENTS.md`。
2. 本 prompt 关联 plan，以及：
   - `docs/superpowers/plans/2026-10-28-panel-community-mode-contract-plan.md`
   - `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`
   - `docs/reference/REDLINES.md`
   - `docs/reference/ARCHITECTURE.md`
   - `docs/reference/FEATURE_LOCATOR.md` §6.11
   - `docs/reference/TERMINAL_RUNTIME.md`
3. `interfaces/webchat/src/panel/{source,projection,registry,loader,shell,sdk}.rs`。
4. `interfaces/webchat/src/panel/loader/browser.rs`、`interfaces/webchat/src/app.rs`、`interfaces/webchat/index.html`。
5. `interfaces/webchat/panel_loader.js`、`interfaces/webchat/tests/panel_loader.test.mjs`、`interfaces/webchat/tests/panel-spike.spec.ts` 与现有 Panel build/artifact/protocol/host fixtures。
6. 查找并读取适用的测试 skill；不要重新 brainstorming 已批准的 source → snapshot → projection 架构。

## Current facts / 当前事实

- `PanelUiCapabilityDeclaration` 已携带 `asset_integrity`，`PanelProjection` 已验证社区 SRI、trust、permissions、versioned relative entry、public base、collision 和 revision。
- 当前 projection 输出仍只有既有 `PanelTabDescriptor`；已验证 integrity 尚未贯通到 `TabRegistry` / `PanelLoader`。
- `PanelLoader` 已具备 generation + active key + exact descriptor fencing，但 same descriptor 的 integrity replacement 尚需纳入 attempt identity。
- `panel_loader.js` 当前直接 `import()` entry；社区资产尚无 fetch/hash/import 边界。
- 首版社区 trust 仍是显式 `PanelUiTrust::Trusted`；same-origin module 不是安全沙箱。

## Required outcome / 必须产出

- 已验证的 optional integrity 只通过现有 projection → registry → loader 路径传递；不得新增第二份 descriptor、registry、loader 或权限模型。
- built-in Terminal 保持现有直接 import 行为；只有带已验证 integrity 的社区模块进入 fetch → SHA-256/SRI verify → import 路径。
- 社区首版只支持自包含单文件 ES module fixture；不支持相对 import、外部 WASM、远程 URL 或动态资产构建。
- HTTP error、redirect、malformed integrity、hash mismatch 必须 fail-closed，且 mismatch 前不得执行 module top-level/default initializer。
- 验证成功后才能 import；Blob URL 必须有确定 cleanup，mount/unmount、SDK 权限和既有 loader generation 语义保持不变。
- same key + same descriptor 但 integrity 改变的 snapshot replacement 必须使旧 load attempt 失效；卸载/移除后不得遗留或复用旧 integrity。
- 新增稳定非 secret load error code，并贯通 JS → WASM bridge → Rust `PanelLoadError`。
- 保留 Terminal descriptor/route/entry/Shell/SDK/Loader 与现有 Panel fixtures；不连接真实 Gateway。

## Hard constraints / 硬约束

- 遵守 R1/R2/R4/R8/R9/R10；不修改 `src/harness/`、PTY server business logic、Gateway Bound Stage C、production ZahirFacade mount 或默认 release。
- 不实现 install/update/uninstall、marketplace、签名服务、production ExtensionManager/discovery adapter、后台 refresh 或真实社区安装流程。
- 不在 production `app.rs` 挂 fixture source，不新增社区 Tab route，不宣称已有可见的 production 社区 UI。
- 不放宽既有 entry/public-base/trust/permission 校验；不加载未经验证的社区字节，不把 WASM 或 same-origin 当作沙箱。
- 不引入新的前端 framework；优先复用 WebCrypto、Fetch、Blob 和现有 loader。
- 中文沟通，代码注释英文，文档中英双语；文档不得写死机器绝对路径。
- 每个 cargo 子进程前从 worktree root 执行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从所属 package 目录串行执行。
- 不运行真实 Gateway，不产生、保存或输出 credential、token、session id、PTY id。loopback/artifact fixture 不得称为 live Gateway evidence。

## Verification and handoff / 验证与交付

按实际修改运行：memory gate、受影响 Rust tests/check/clippy、loader Node tests、社区 integrity browser fixture、现有 Panel build/artifact/protocol/host fixtures、`git diff --check` 和独立只读 review。

最终报告必须明确：已验证的加载与 fencing 事实、Terminal 兼容性、单文件限制、未实现的安装/更新/卸载/discovery/WASM 能力、未运行真实 Gateway，以及实际测试与 skipped 项。提交建议：

`feat(panel): verify community module assets before loading`

English handoff: continue from `main` after `aab26f257` in a fresh `panel-community-loader` worktree. Thread the already validated SRI metadata through the existing projection/registry/loader, verify exact community module bytes before import, and prove failure and late-result fencing with local fixtures only. Do not add installation, production discovery, arbitrary multi-asset loading, a default-release flip, merge, or push.
