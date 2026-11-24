# 下一阶段重启 Prompt / Next-Phase Restart Prompt

继续实施 Panel 社区 UI 的下一阶段：**把当前已验证的进程内 `PanelAssetPublicationStore` 接入一个受认证、受 canonical declaration 约束的 production publication route，并让显式选择的 community Panel 通过真实 browser asset delivery 使用现有 `PanelLoader` / `PanelShell` 生命周期。**

当前阶段的目标是补齐上一阶段明确 deferred 的 production HTTP publication 与浏览器认证注入边界；snapshot discovery 仍然保持 passive，任何 snapshot arrival 都不得自动选择、import 或 mount。

## Starting point / 起点

- 当前 `main` 已合并 `panel-community-assets`，HEAD 为 `fd47b8805`（`merge: integrate panel community assets mount`）。
- 已完成并保留的契约：
  - Core `ExtensionManager::panel_ui_capability_snapshot()` 是唯一 declaration/snapshot/source-revision authority；
  - authenticated `/ws` JSON-RPC `panel.capabilities.snapshot` discovery transport；
  - `PanelAssetPublicationStore` 的 canonical identity、exact bytes、SRI、replacement/retraction 和 stale revision fencing；
  - 显式 `/panel/<plugin_id>/<tab_id>` selection；
  - 现有 `PanelProjection`、`PanelLoader`、`PanelShell` 的 descriptor/integrity/async cleanup fencing；
  - deterministic `panel-tab-spike` fixture，以及双语 validation report。
- 上一阶段明确未完成：production HTTP asset route、真实 browser credential propagation、live Gateway asset delivery。`PanelAssetRequest.authorized: bool` 只是未挂载 in-process seam 的 caller verdict，不能直接成为 production authorization。
- 先读并以其为事实源：
  - `docs/superpowers/plans/2026-10-30-panel-community-assets-live-publication-plan.md`
  - `docs/reports/panel-community-assets-mount-validation.md`
  - `docs/reference/FEATURE_LOCATOR.md` §6.11
  - `docs/reference/TERMINAL_RUNTIME.md`
  - `docs/reference/REDLINES.md`, `docs/reference/SECURITY.md`
  - `shared/protocol/src/panel.rs`
  - `src/gateway/server/panel_asset_route.rs`
  - `src/gateway/server/mod.rs` and existing artifact/canvas/browser-viewer HTTP guards
  - existing `SharedTokenManager`, `DeviceTokenManager`, `OriginPolicy`, `refuse_insecure_remote`, `resolve_client` and MCP auth seams
  - `src/extension/{panel,projection,lifecycle,manifest,types}.rs`
  - `interfaces/webchat/src/panel/{discovery,projection,registry,loader,shell,source}.rs`
  - `interfaces/webchat/src/{context,app}.rs`
  - `interfaces/webchat/scripts/build-panel-spike.ts` and existing Node/Playwright fixtures
- 从当前 `main` 创建独立 worktree/branch，例如 `panel-community-live-publication`。不要直接编辑共享 `main`，不要 merge/push，除非收到新的明确授权。
- 使用 `superpowers:executing-plans` 顺序执行；每个实现 task 先写最小 RED，确认失败原因后再写 production code。

## Hard boundaries / 硬边界

- Core 仍是唯一 declaration、trust、snapshot 和 source-revision authority。
- Production route 必须从 route-owned authenticated request context 得到 authorization；不得接受 caller-supplied `authorized: bool` 作为最终授权。
- 不暴露 raw plugin root、arbitrary filesystem path、credential、arbitrary URL、remote fetch、external WASM 或 multi-file asset graph。
- route 只允许 canonical identity 和由 admitted declaration 派生的 versioned `./tabs/` single-file entry；不得接受 client-selected revision、manifest fields、generic plugin entry 或任意 client path。
- 复用现有 `PanelAssetPublicationStore` 和 `PanelLoader` 的 exact-byte/integrity 语义；不得新增第二个 loader、registry、trust model 或 revision source。
- HTTP route 必须显式复用 transport/origin/TLS/auth guards；`/ws` authentication 不会自动转移到 HTTP GET。
- snapshot discovery 必须保持 passive；只有显式 `/panel/<plugin_id>/<tab_id>` selection 才能进入 route fetch、module import 或 `PanelShell` mount。
- `/terminal` 与现有 built-in release 行为保持不变。
- 不实现 install/update/uninstall、marketplace、signature service、background refresh、live notification、default-release 切换、Zahir runtime mount 或完整 multi-file dependency graph。
- 如果无法找到一个由 Core-admitted state 驱动、且可绑定真实认证上下文的安全 publication producer，停在 audit/design finding，不用新 endpoint 绕过边界。
- 每次 Cargo 前从 worktree root 执行 `python qa/browser_live/check_memory.py`；Cargo 从 owning package directory 串行运行。
- 中文沟通，English code comments，双语文档；仓库文档不得写机器绝对路径。

## Required work / 必须完成

1. **Production ownership and auth audit + RED contract**
   - 确认 admitted declaration、publication bytes、HTTP public address、route auth 和 browser credential 的唯一 owner。
   - 具体说明如何把 route request 绑定到既有 authenticated client/device/shared token，而不把 broad `GatewaySharedState` 变成新的 Panel authority。
   - 写 RED tests：unauthenticated/invalid-origin/insecure-remote requests fail closed；authorized request 只能读取 canonical identity/current revision/exact entry；client path、URL、revision、generic fields 和 raw filesystem root 均无法改变 publication result；stale/retracted/replaced records fail closed。
   - 先验证现有 HTTP auth seam 能否安全复用；不能安全复用时只提交 design finding。

2. **Route-owned authenticated publication**
   - 将 `PanelAssetRequest.authorized: bool` 替换为 route-owned authenticated context 或等价的不可伪造内部授权证明。
   - 将 canonical admitted publication store 通过窄 route state 注入真实 Gateway route；不得从 client path 解析 plugin root，不得重新解析 manifest/trust/revision。
   - 复用 `OriginPolicy`、`refuse_insecure_remote`、`resolve_client`、现有 bearer/device token validation 和 private rate-limit 模式；generic refusal 不泄漏文件是否存在、路径或 credential。
   - 只返回已经 admission 且 SRI/bytes 一致的 single-file module；覆盖 unauthorized、origin/TLS、missing、stale、retraction、replacement、entry mismatch 和 exact bytes。

3. **真实浏览器 delivery 与显式 mount**
   - 让 explicit community selection 使用当前 snapshot 的 canonical `public_base`/entry，通过 authenticated same-origin asset request 到达现有 exact-byte loader。
   - 不让 discovery 自动触发 fetch/import/mount；保留 `/panel/<plugin_id>/<tab_id>` 显式选择。
   - 覆盖 disconnect、retraction、revision replacement、descriptor change、route refusal、loader failure、unmount failure 后的 late-result fencing 和一次性 cleanup。
   - 保持 built-in Terminal source/projection/registry/loader 测试绿色。

4. **Deterministic + Gateway integration fixture**
   - 保留现有 `panel-tab-spike` deterministic fixture，并新增最小 route integration fixture，证明：authenticated request → canonical publication record → exact bytes → existing loader → explicit `PanelShell` mount → cleanup。
   - fixture 必须区分 deterministic in-process proof 与真实 Gateway/auth/browser proof；不得用 fake JSON-RPC endpoint 宣称 production parity。
   - 在 generated artifact、Gateway service、browser credential 或 Windows 环境缺失时记录精确阻塞边界。

5. **Bilingual docs, review and delivery**
   - 更新 `FEATURE_LOCATOR.md` §6.11、必要的 `TERMINAL_RUNTIME.md`，新增/更新 production publication validation report。
   - 报告分别说明 discovery、in-process publication、route-owned auth、真实 browser delivery、explicit mount 已证明的内容，以及 install lifecycle/default release/live notification 等仍 deferred 的内容。
   - 运行 fresh read-only review，重点审查 auth binding、origin/TLS guard、path/URL/filesystem exposure、integrity-to-bytes、revision fencing、selection bypass、auto-mount、stale async cleanup、duplicate authority 和 Core→WASM dependency direction。

## Delivery / 交付

保留每个 task 的 RED/GREEN 证据、conventional commits、plan ledger 和最终 tracked-clean 状态。不要在本阶段 merge 或 push，除非后续获得明确授权。最终交付必须包含：worktree/branch、commit 列表、实际验证结果、未执行/阻塞检查、独立 review 结果、route auth 的明确证明，以及下一阶段仍 deferred 的边界。

English handoff: starting from `main` at `fd47b8805`, create a fresh `panel-community-live-publication` worktree. Replace the unbound authorization verdict with route-owned authentication, wire the canonical admitted publication store into a narrow HTTP asset route, and deliver the explicitly selected community Panel through the existing integrity-gated loader and PanelShell lifecycle. Keep discovery passive, preserve Terminal, reject path/URL/filesystem/revision/integrity violations, and stop before installation lifecycle, default release, live notifications, multi-file assets, external WASM, or Zahir runtime mount.
