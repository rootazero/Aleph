# 下一阶段重启 Prompt / Next-Phase Restart Prompt

实施 Panel 社区 UI 的下一阶段：**在已完成的 canonical snapshot transport 之上，先审计并实现受约束的 asset publication，再把显式选择的 community Panel 接入现有 `PanelShell` / `PanelLoader` mount 生命周期。** 当前阶段不得把 snapshot 到达自动解释为 route selection 或 mount。

## Starting point / 起点

- 当前 `main` 已合入 `panel-community-transport`，HEAD 为 `c3c9e25d6`。
- 已完成的 transport contract 是：现有 authenticated WebSocket `/ws` JSON-RPC 控制面、`panel.capabilities.snapshot`、Core `ExtensionManager::panel_ui_capability_snapshot()` 唯一 authority、webchat discovery-only state、connection epoch + refresh request + revision fencing。
- 先读并以其为事实源：
  - `docs/superpowers/plans/2026-10-30-panel-community-assets-mount-plan.md`
  - `docs/reports/panel-community-snapshot-transport-validation.md`
  - `docs/reference/FEATURE_LOCATOR.md` §6.11
  - `docs/reference/TERMINAL_RUNTIME.md`
  - `docs/reference/REDLINES.md`
  - `shared/protocol/src/panel.rs`
  - `src/extension/{manifest,panel,projection,lifecycle,mod}.rs`
  - existing Gateway/static artifact/browser-viewer handlers and path/auth guards
  - `interfaces/webchat/src/panel/{source,discovery,projection,registry,loader,shell}.rs`
  - `interfaces/webchat/src/app.rs` and existing Panel Node/Playwright fixtures
- 从当前 `main` 创建独立 worktree/branch `panel-community-assets`。不要直接编辑共享 `main`，不要 merge/push，除非收到新的明确授权。
- 使用 `superpowers:executing-plans` 顺序执行；每个实现 task 先写最小 RED，确认失败原因后再写 production code。

## Hard boundaries / 硬边界

- Core 仍是唯一 declaration/snapshot/revision authority。
- WASM 不得依赖 Core、Server、`PluginManifest` 或 `ExtensionManager`。
- 复用现有 authenticated/static publication seam 和已经完成的 integrity-gated single-file `PanelLoader`；不得新增第二个 loader、registry、trust model 或 revision source。
- 不暴露 raw plugin path、arbitrary filesystem path、credential、arbitrary URL、external WASM 或 multi-file asset graph。
- snapshot discovery 必须保持 passive；只有显式用户/调用方 selection 才能进入 route selection、loader 或 `PanelShell` mount。
- 不自动挂载第一个 community declaration，不改变 Terminal/default release，不实现 install/update/uninstall、marketplace、signature service、background refresh、live notification 或 live Gateway parity。
- 如果现有 publication 或 explicit selection seam 不能安全复用，停在 audit/design finding，不通过新 endpoint 绕过边界。
- 每次 Cargo 前从 worktree root 执行 `python qa/browser_live/check_memory.py`；Cargo 从 owning package directory 串行运行。
- 中文沟通，English code comments，双语文档；仓库文档不得写机器绝对路径。

## Required work / 必须完成

1. **Ownership audit + RED contract**
   - 找到现有 asset bytes/public address owner 和显式 route/selection/PanelShell owner。
   - 记录是否可复用现有 authenticated/static seam，以及为什么不需要新 endpoint 或 DTO。
   - 写 RED tests：exact bytes/integrity、traversal/arbitrary URL/unauthorized/missing asset fail closed、snapshot arrival does not mount、explicit selection is required。

2. **Asset publication bridge**
   - 只发布 admitted、受路径约束、由 canonical declaration 指向的 single-file module。
   - 复用现有 auth/path checks；不重新解析 manifest、不重新做 trust decision、不生成 revision。
   - 覆盖 bytes mismatch、integrity mismatch、retraction/replacement、stale descriptor、auth failure、path escape 和 generic plugin fields。

3. **Explicit browser mount**
   - 让显式 selection 验证 descriptor identity、snapshot revision/source revision 和当前 publication，再调用现有 `PanelProjection`/registry/`PanelLoader`/`PanelShell`。
   - 保持 built-in Terminal 路径不变。
   - 覆盖 deselect/disconnect/retraction/replacement/loader failure/unmount failure 后的 late-result fencing 和一次性 cleanup。
   - 禁止 snapshot arrival 自动 route selection、自动 import 或自动 mount。

4. **Deterministic fixture and verification**
   - 建立 `canonical snapshot → explicit selection → publication seam → exact-byte loader → PanelShell mount → cleanup` fixture。
   - 明确标注 deterministic fixture，不宣称 live Gateway parity。
   - 运行 affected Rust/Node/browser checks；缺少 generated artifact 时记录精确边界。

5. **Bilingual docs and review**
   - 更新 `FEATURE_LOCATOR.md` §6.11、必要的 `TERMINAL_RUNTIME.md`，新增 `docs/reports/panel-community-assets-mount-validation.md`。
   - 报告分别说明 transport、publication、explicit selection/mount 已证明的内容，以及 install/update/default-release/live-notification/live-Gateway 仍 deferred 的内容。
   - 运行 fresh read-only review，重点审查 path traversal、arbitrary URL/filesystem exposure、integrity mismatch、selection bypass、auto-mount、stale async mount、duplicate authority 和 Core-to-WASM dependency direction。

## Delivery / 交付

保留每个 task 的 RED/GREEN 证据、conventional commits、ledger 更新和最终 tracked-clean 状态。不要在本阶段合并或推送。最终交付必须包含：worktree/branch、提交列表、实际测试结果、未执行/阻塞检查、独立 review 结果和明确的下一阶段边界。

English handoff: continue from `main` at `c3c9e25d6` in a fresh `panel-community-assets` worktree. Audit the existing asset publication and explicit browser selection owners, then add a constrained publication bridge and an explicit community Panel mount using the existing integrity-gated loader and PanelShell lifecycle. Keep discovery passive, preserve Core as the only snapshot authority, reject path/URL/integrity/identity violations, and stop before installation lifecycle, default-release changes, live notifications, or live Gateway claims.
