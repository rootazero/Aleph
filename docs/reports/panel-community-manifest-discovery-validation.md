# Panel Community Manifest Discovery Validation

> Date / 日期: 2026-10-29
> Scope / 范围: explicit community Panel manifest declaration and canonical discovery snapshot
> Worktree / 工作树: `panel-community-discovery`
> Commits / 提交: `8f8a6e359`, `40b4bd190`, `5dbf0b990`, `44d51f56b`, `4b958f345`

## Intent / 目的

This phase makes community Panel UI an explicit, auditable manifest declaration and derives the existing canonical Panel capability snapshot from admitted extension state. It stops at the discovery contract. It does not publish community assets, add a browser route, mount a browser module, or implement installation lifecycle.

本阶段把社区 Panel UI 从通用插件字段的隐式世界提升为显式、可审计的 manifest declaration，并从已准入扩展状态派生现有 canonical Panel capability snapshot。阶段边界停在 discovery contract，不发布社区资产、不增加浏览器路由、不挂载浏览器 module，也不实现安装生命周期。

## Contract / 契约

The optional `panel` section is accepted by the JSON, Claude TOML, and native Aleph TOML manifest forms. Its declaration facts are:

- `tab_id`, `label`, `icon`, and `order`;
- relative single-file `entry`;
- supported `api_major`;
- Panel SDK `permissions` only;
- required `sha256-<base64>` `asset_integrity`.

`public_base` is host-owned and is projected as `./tabs/`; the manifest cannot choose it. The manifest also cannot choose trust, origin, owner generation, active status, filesystem permissions, credentials, runtime handles, or arbitrary URLs. Unknown fields are rejected through the strict schema. A Panel section without a non-empty plugin `version` is rejected.

`PluginRecord.panel` retains the explicitly parsed declaration. The pure adapter in `src/extension/panel.rs` derives one `PanelUiCapabilityDeclaration` from the admitted record, the existing `OwnerTrustPolicy`, and the lifecycle snapshot revision. It derives the plugin id, version, origin, trust, status, canonical versioned entry, public base, and source revision from host state and policy. Generic `entry`, plugin `kind`, generic permissions, `capabilities_v2`, and `wasm_capabilities` never create a Panel declaration.

The shared wire owner is `shared/protocol/src/panel.rs`. `PanelUiCapabilityDeclaration`, `PanelUiCapabilityOrigin`, and `PanelUiCapabilitySnapshot` are the only serializable declaration/snapshot shapes. Webchat consumes those protocol types through its existing `PanelUiCapabilitySource`; it does not import Core or `PluginManifest`.

## Policy and fail-closed behavior / 策略与 fail-closed 行为

A record must be active (`Loaded` or `Pending`), have a non-empty version, carry an explicit valid Panel declaration, pass `OwnerTrustPolicy`, and satisfy the shared identity, relative-entry, permission, API-major, and SRI format validators. Invalid rows are omitted with stable `panel.*` diagnostics. A duplicate qualified key omits the colliding group rather than selecting one winner. The adapter never assigns `Verified`; community rows receive host policy trust (`Trusted`) and `Community` origin. Browser projection repeats its own boundary validation and rejects unacceptable trust, origin, permissions, integrity, public base, entry, version prefix, duplicate, collision, or revision mismatch.

记录必须处于 active 状态（`Loaded` 或 `Pending`）、有非空 version、带显式合法 Panel declaration、通过 `OwnerTrustPolicy`，并通过共享 identity、relative-entry、permission、API-major 与 SRI 格式校验。无效行以稳定 `panel.*` diagnostic 被省略。qualified key 冲突时整组省略，不选择一个赢家。adapter 不会赋予 `Verified`；community row 的 trust (`Trusted`) 与 origin (`Community`) 由宿主策略派生。浏览器 projection 继续在边界重复校验并拒绝不接受的 trust、origin、permission、integrity、public base、entry、version prefix、duplicate、collision 或 revision mismatch。

## Revision and retraction / revision 与撤回

`ExtensionManager -> Views -> lifecycle.rs::after_transition` remains the only projection trigger. `Views::republish_plugin_projections` increments the Panel revision and replaces the complete `PanelUiCapabilitySnapshot`. Loading or replacement publishes a new revision; disabling, unloading, parse/error status, or removal publishes a new snapshot without the old declaration. No second Panel registry, watcher, revision authority, or transport channel was added.

`ExtensionManager::panel_ui_capability_snapshot()` and `panel_ui_capability_revision()` expose the canonical Core-side read model for a later transport owner. No production HTTP/WS route or browser discovery call was added in this phase.

`ExtensionManager -> Views -> lifecycle.rs::after_transition` 仍是唯一 projection trigger。`Views::republish_plugin_projections` 递增 Panel revision，并整体替换 `PanelUiCapabilitySnapshot`。load 或 replacement 发布新 revision；disable、unload、parse/error status 或 removal 发布不含旧 declaration 的新 snapshot。本阶段没有第二个 Panel registry、watcher、revision authority 或 transport channel。

`ExtensionManager::panel_ui_capability_snapshot()` 与 `panel_ui_capability_revision()` 暴露 Core 侧 canonical read model，供后续 transport owner 使用。本阶段没有生产 HTTP/WS route，也没有浏览器 discovery call。

## Verification / 验证

All Cargo commands were preceded by `python qa/browser_live/check_memory.py` from the worktree root. Cargo was run serially from the owning package directory.

所有 Cargo 命令均先从 worktree root 执行 `python qa/browser_live/check_memory.py`，并从 owning package directory 串行运行 Cargo。

| Check / 检查 | Result / 结果 |
|---|---:|
| `cargo fmt` and `cargo fmt --check` | pass |
| `cargo test panel` from `shared/protocol` | 9 passed, 0 failed, 398 filtered |
| `cargo check --lib` from worktree root | pass; source typechecks |
| `cargo check --tests` from worktree root | pass; adapter, lifecycle, and fixture test code typechecks |
| `cargo clippy --lib` from worktree root | pass; no warnings |
| Node loader regression (`tests/panel_loader.test.mjs`) | 12 passed, 1 skipped (generated glue), 0 failed |
| Node protocol/host artifact fixtures | blocked: fresh artifact build reached WASM output but failed at `bun run --bun tailwindcss` because the local `tailwindcss` script is unavailable; direct fixture invocation correctly rejected missing `PANEL_OUTPUT_ROOT` |
| `git diff --check` | pass before documentation changes |

The focused `cargo test --lib manifest::panel_` reached the `alephcore` test binary link but did not execute assertions because the Windows LLVM linker failed with `rustc-LLVM ERROR: out of memory`, `Allocation failed`, and exit `0xc0000409, STATUS_STACK_BUFFER_OVERRUN`. This is recorded as an environment/linker limitation; the corresponding source and test code typechecked under `cargo check --tests`.

窄范围 `cargo test --lib manifest::panel_` 已到达 `alephcore` 测试二进制链接阶段，但由于 Windows LLVM linker 报 `rustc-LLVM ERROR: out of memory`、`Allocation failed`，并以 `0xc0000409, STATUS_STACK_BUFFER_OVERRUN` 退出，未执行 assertion。该结果记录为环境/linker 限制；对应 source 与 test code 已由 `cargo check --tests` 完成类型检查。

The Node loader regression (`tests/panel_loader.test.mjs`) passed 12 tests and skipped one test that requires generated `ALEPH_PANEL_BRIDGE_GLUE`. The protocol and host artifact fixtures were not claimed as passed: `bun run panel:tabs:build -- --clean --output=.panel-tabs-discovery` successfully built the two WASM modules but stopped at `bun run --bun tailwindcss` because this checkout has no local `tailwindcss` script. Running the fixture files without a fresh output correctly failed with `PANEL_OUTPUT_ROOT must name the newly built isolated output`. No dependency installation was performed.

Node loader regression（`tests/panel_loader.test.mjs`）通过 12 项测试，另有 1 项因需要生成的 `ALEPH_PANEL_BRIDGE_GLUE` 被 skip。protocol 与 host artifact fixture 没有宣称通过：`bun run panel:tabs:build -- --clean --output=.panel-tabs-discovery` 已成功生成两个 WASM module，但在 `bun run --bun tailwindcss` 处因 checkout 中没有本地 `tailwindcss` script 停止。未提供 fresh output 直接运行 fixture 时，测试按契约报 `PANEL_OUTPUT_ROOT must name the newly built isolated output`。本阶段未安装依赖。

## Deferred boundaries / 延后边界

Deferred to a later approved phase:

- asset publication/server and public hosting;
- production transport for the Core snapshot;
- browser route or `PanelShell` community mount;
- installation, update, uninstall, marketplace, signature service, and background refresh;
- remote URLs, multi-file asset graphs, relative imports, external WASM, and sandboxing;
- default release changes, ZahirFacade runtime mounting, and real Gateway/live evidence.

以下内容延后到后续单独批准阶段：

- asset publication/server 与 public hosting；
- Core snapshot 的生产 transport；
- 浏览器 route 或 `PanelShell` community mount；
- installation、update、uninstall、marketplace、signature service 与 background refresh；
- remote URL、多文件 asset graph、relative import、external WASM 与 sandboxing；
- default release 变更、ZahirFacade runtime mount 与真实 Gateway/live evidence。

The local JSON fixture in `interfaces/webchat/src/panel/projection.rs` proves protocol-shape consumption by the existing webchat projection. It is discovery evidence only and does not claim that a community tab is visible in a production browser.

`interfaces/webchat/src/panel/projection.rs` 的本地 JSON fixture 证明现有 webchat projection 能消费 canonical protocol shape。它只构成 discovery evidence，不声称生产浏览器中已经可见社区 tab。
