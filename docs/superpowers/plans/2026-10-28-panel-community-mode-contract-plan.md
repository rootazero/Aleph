# Panel 社区插件自由模式：Capability / Projection 契约计划

日期：2026-10-28
状态：待执行
前置提交：`c2d8dbf74 test(panel): add terminal build e2e`

## Intent / 目标

为 Panel UI 建立一个面向社区插件的、可验证的能力声明与 projection seam，使内置 Terminal 和未来社区 Tab 能进入同一个受控的 PanelShell / PanelSdk / PanelLoader 生命周期。第一阶段只建立契约、来源适配和 fail-closed 验证，不实现插件安装、资产构建、动态挂载或真实 Gateway 生命周期。

最终目标是让社区能够以自由模式接入 Panel UI，同时保留 Rust Core 唯一事实源、Bridge IPC 边界、Capability 约束、资产完整性和生命周期所有权。此计划只完成通往该目标的第一条可审计路径。

## Current state / 当前事实

- `PanelProjection::builtins(...)` 当前只产生 host-owned Terminal descriptor。
- `TabRegistry` 已负责 descriptor 校验、重复 ID 防护、entry/base 解析和 descriptor snapshot，但没有社区插件来源。
- `PanelLoader` / `PanelShell` / `PanelSdk` 已形成完整的 built-in Terminal 生命周期；`PanelSdk` 通过 transport 发送 `events.subscribe`，Terminal module 只消费严格的 epoch/topic。
- 当前没有 production `ExtensionManager -> PanelProjection` adapter，也没有 production `panel.*.describe` discovery RPC。
- `ZahirFacade` runtime mount 和其他 Capability backend 仍是独立 deferred scope；本任务不得借机启用它们。
- 已有 build E2E 证明当前 host/Terminal 资产链路，但 live Gateway 仍是 `BLOCKED`。

## Goals / 目标

1. 定义一个唯一的 Panel UI capability declaration / snapshot 契约，表达 descriptor、来源、版本、完整性、权限和 snapshot revision。
2. 让 `PanelProjection` 从一个明确的 source/snapshot 得到内置和未来社区 Tab 的统一 projection；不再让 app 层自行拼第二份 registry。
3. 在 projection 边界一次性验证未知来源、重复 ID、内置 Tab 冲突、非法 entry/base、缺失完整性、未知权限和不支持的 trust 状态，并保持 fail-closed。
4. 保留现有 Terminal 行为和 production-host loopback evidence；新增 fixture 证明社区样例可以被发现、验证、排序、投影和拒绝。
5. 为下一阶段插件资产加载和生命周期管理留下稳定的 revision、owner 和 cleanup 边界。

## Non-goals / 本阶段不做

- 不实现插件安装、更新、卸载、签名服务或社区 marketplace。
- 不构建或复制社区插件 JS/WASM/CSS，不改变 `build-panel-tabs.mjs` 的部署资产协议。
- 不启用 production `ZahirFacade` mount，不新增 Capability runtime ownership，不修改 `src/harness/`。
- 不把不可信社区资产直接加载进 same-origin production host；本阶段只验证 declaration 和 projection。
- 不迁移第二个内置 Tab，不修改 PTY server、Gateway Bound Stage C、community lifecycle 或默认 release。
- 不运行真实 Gateway；本阶段的插件样例只使用本地 fixture/source。

## Approaches / 方案比较

### A. 让 `ExtensionManager` 直接调用 `PanelShell`

实现速度快，但会让 Core/extension 层直接知道浏览器 UI 生命周期，违反 R1/R2，且把 source、owner 和 browser registry 混成多个事实源。放弃。

### B. Capability declaration → snapshot source → PanelProjection（推荐）

在 Core/Bridge 边界定义可验证的 Panel UI capability declaration；source 只提供带 revision 的 snapshot，`PanelProjection` 负责一次验证并输出现有 `PanelTabDescriptor` projection。当前只接内置 source 和 fixture source，未来再接 ExtensionManager。这样保留单一 projection、可审计的 fail-closed 边界，并能逐步接入社区插件。

代价是第一阶段不会产生可见的社区插件 UI，需要后续独立完成资产、信任和生命周期；这是为了避免先把不安全的动态模块加载接入生产 host。

### C. iframe / WebComponent 沙箱

可以提前隔离社区 UI，但会引入第二套消息、主题、权限和 teardown 模型，且与现有 PanelShell/PanelSdk/PanelLoader 的真实生命周期分叉。除非后续安全模型明确要求跨 origin 隔离，否则不采用。

## Design decisions / 设计决定

- 现有 `PanelTabDescriptor` 继续作为浏览器侧唯一 projection shape；不要再创建一份只供浏览器消费的平行 descriptor。
- 新增的 declaration 只包裹 projection 所需 descriptor，并携带 `origin`、`trust`、`plugin_id/version`、`asset_integrity`、`permissions` 和 source snapshot revision；字段的最终命名须复用现有 schema/type 风格。
- `PanelUiCapabilitySnapshot` 是 source 的唯一输出，包含稳定 `revision` 和声明集合。source 不暴露可变 registry，也不让浏览器反向发现插件。
- source 顺序固定为 host built-ins，再按稳定 plugin identity 排序；同一 ID、内置 ID 冲突或同一 plugin 的不一致 revision 均 fail-closed。
- 社区 declaration 必须具有受约束的 versioned relative entry、明确 integrity 和受支持的 permission set；外部 URL、路径穿越、空 identity、未知 trust 或未声明权限一律拒绝。
- projection 只返回已经验证的 descriptor；拒绝结果必须包含稳定的非 secret reason code，不能把拒绝转换成成功的 no-op。
- snapshot revision 必须贯穿 source → projection → registry，旧 snapshot 的 late result 不能覆盖新 snapshot；本阶段不做后台 refresh，只测试替换和 fencing。
- 当前 built-in Terminal source 的输出必须与现有 `PanelProjection::builtins(...)` 完全一致，避免改变 terminal route、descriptor identity、asset entry 或 PanelSdk transport。
- ExtensionManager 生产接入留在后续阶段；本阶段用 in-process fixture source 证明契约，而不是伪造社区安装成功。

## Work breakdown / 工作拆分

### 1. Contract and boundary inventory

阅读并记录现有 `PanelTabDescriptor`、`TabRegistry`、`PanelProjection`、`PanelShell`、`PanelLoader`、`PanelSdk`、ExtensionManager 和 Capability descriptor 的实际字段与 ownership。确认新 declaration 不重复已有事实；若需要跨 crate wire type，先确定唯一 schema owner 和依赖方向。

### 2. Add the declaration and snapshot seam

实现最小的 `PanelUiCapabilityDeclaration`、`PanelUiCapabilitySnapshot` 和 source trait/adapter。把 built-in Terminal 迁移到该 source seam，保持 `PanelProjection::builtins` 的外部行为兼容。不要接真实 ExtensionManager，不要添加 runtime mount。

### 3. Validate and project

在 projection 边界完成一次验证：identity、origin、trust、permissions、integrity、relative entry、public base、revision、duplicate/collision 和 deterministic ordering。验证结果必须是明确的 accepted projection 或结构化 rejection。

### 4. Fixture and regression coverage

增加 fixture source，覆盖：一个合法 community declaration、重复 ID、builtin collision、非法 entry、缺失 integrity、未知 permission、旧 revision late result、不可接受 trust。保留现有 Terminal component/build/host fixtures并证明内置行为无回归。

### 5. Documentation and handoff

更新 `FEATURE_LOCATOR`、`TERMINAL_RUNTIME` 或其他适用 reference，只记录已验证事实；明确本阶段仍未实现 plugin install/assets/runtime mount/live Gateway。生成下一阶段入口：资产完整性与受控 module load 的独立 plan。

## Acceptance criteria / 验收标准

- Rust/Protocol/Server/Client 依赖方向没有违反仓库 AGENTS 约束；若修改 public Protocol 或 Server `HttpApi`，运行 `bun run generate` from `packages/client`。
- 内置 Terminal 的 descriptor identity、entry、route、PanelShell/PanelSdk/PanelLoader 行为与当前 baseline 一致。
- 所有社区声明只能经单一 source → snapshot → projection → registry 路径进入 Panel；没有第二份浏览器 registry 或 app 层旁路。
- 合法 declaration 可被稳定投影；每个非法样例都有确定 rejection reason；不存在报成功的 no-op。
- 新旧 snapshot 替换后，旧 revision 的 late result 不能覆盖新状态，也不能重新挂载已拒绝或已卸载的 descriptor。
- 没有社区 JS/WASM 被加载，没有真实 Gateway 操作，没有 production ZahirFacade mount。
- 运行适用的 Rust tests/check/clippy、现有 Panel Node fixtures、`git diff --check`，并记录未运行项。
- worktree clean，使用 conventional commit；不 merge/push/default release flip。

## Deliverables / 交付物

- Panel UI capability declaration/snapshot contract and source adapter。
- Projection validation and deterministic rejection tests。
- Built-in Terminal compatibility tests plus fixture-only community declaration tests。
- 双语 design/validation note，明确后续资产加载、信任、安装和生命周期阶段。
