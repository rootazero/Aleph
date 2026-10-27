# Panel UI modularization — next-stage restart prompt

实施 Panel UI 模块化的下一阶段：**通用 Panel Shell、TabRegistry 与 UI descriptor projection 收口，为第二个 built-in Tab 迁移准备稳定边界**。

本会话承接 Terminal host acceptance。Terminal 已完成 component fixture 与 production host/loopback wire fixture，但真实 Aleph Gateway parity 仍是独立的 BLOCKED 事项；不要把 loopback fixture 重命名为 live evidence，也不要让 live Gateway 缺少成为本阶段通用 Shell 工作的阻塞。

## Current distance / 当前距离

不要给整体项目编造百分比。按设计 Stage 表述：

- **Terminal 首个 Tab 垂直切片：已完成到 production host/wire acceptance。** 真实 host WASM、`PanelShell`、`PanelSdk`、版本化 terminal WASM 和 loopback WebSocket fixture 已通过；component fixture 覆盖 `/` 与 `/panel/` public base。
- **真实 Gateway/live parity：未完成，BLOCKED。** 当前没有授权的 disposable Aleph Gateway/project/session/credential；没有接触用户 Gateway 或 PTY/session。
- **通用 Shell/registry/projection：部分存在但尚未收口。** `TabRegistry`、`PanelLoader`、`PanelSdk` 已存在，但 `interfaces/webchat/src/panel/shell.rs` 仍直接构造单个 `aleph:terminal` descriptor，`interfaces/webchat/src/app.rs` 仍用 `PanelMode::Terminal` 特判进入模块化 Shell。
- **其余 built-in Tabs：尚未迁移。** Chat、Settings、Dashboard、Memory、Agents、Teams、Projects、Extensions 等仍在 legacy host 中。
- **默认 release flip：未完成。** `panel-modular` 仍为 opt-in，legacy 仍是 rollback path。
- **Community UI lifecycle、trust/integrity/update/uninstall、多端 parity、完整 accessibility/leak/release hardening：尚未完成。** 本阶段不实施这些内容。

因此本阶段的目标是把首个 Tab 的一次性特例提升为可复用的 host contract；本阶段完成仍不能宣称整个 Panel UI 模块化完成。

## Starting point / 起点

- 主工作区当前仍在 `main`，HEAD `8b8a70385`，保持干净。
- 已完成的 Terminal host acceptance 提交在独立 worktree：`D:/Workspace/Aleph/.worktrees/terminal-host`，commit `34a3c63bb`：`test(panel): add terminal host acceptance fixtures`。
- 新实现应以 `34a3c63bb` 为起点；如果该 worktree 仍存在，先只读检查其状态，不 force/reset/delete。建议新 worktree/branch 名为 `panel-shell`。
- 不在 `main` 工作区实现；不 merge、push 或切换默认 release，除非本会话收到新的明确授权。

## Read first / 先读

1. 当前 git root 的两级 `AGENTS.md` 与适用 reference。
2. `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`。
3. `docs/superpowers/plans/2026-10-27-panel-terminal-host-acceptance-plan.md`。
4. `docs/reports/panel-terminal-tab-validation.md`，尤其 component / production-host / live 三层 verdict。
5. `docs/reference/TERMINAL_RUNTIME.md`、`docs/reference/FEATURE_LOCATOR.md` 的 §6.11、`docs/reference/SUBSYSTEM_ROUTING.md`、`docs/reference/DEVELOPMENT.md`、`docs/reference/REDLINES.md`。
6. `interfaces/webchat/src/panel/{shell,registry,loader,sdk}.rs`、`interfaces/webchat/src/app.rs`、`interfaces/webchat/src/lib.rs`、`interfaces/webchat/Cargo.toml`、`shared/protocol` 中的 Panel descriptor/types。
7. `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`、`interfaces/webchat/tests/panel-terminal-host.test.mjs`、`interfaces/webchat/tests/fixtures/panel-gateway.mjs`，将 Terminal acceptance 作为回归基线。
8. 当前安装的 graphify skill 指定的 `graphify-out/` 快速定位结果；不要用旧图谱替代源码，也不要为本阶段重建全仓图谱。
9. 查找实际存在的匹配 test skill；没有就记录并按项目现有 Rust/Node/Playwright 约定执行。

使用 `superpowers:brainstorming` 的已批准设计作为背景，使用 `superpowers:executing-plans` 顺序执行本 prompt 关联 plan。不要重新 brainstorming 已批准的总体架构；如果源码证明计划假设错误，先停在计划边界内更新设计和证据，再实现。

## Next-stage intent / 本阶段意图

让 Panel host 从“内置 Terminal 特判 + 单个静态 descriptor”变成“由一个权威的 host-side descriptor projection 初始化 registry，由 route/active key 选择模块，由 loader 负责生命周期”的可复用结构。

本阶段仍只使用已存在的 host-owned built-in descriptors 和 Terminal module。可以增加一个测试用 inert descriptor 或 test-only module contract 来证明 registry/selection 不依赖 Terminal 特判，但不要借机迁移第二个真实 Tab。

## Hard constraints / 硬约束

- 不修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic、community installation/trust chain 或其他 Tabs 的业务逻辑。
- 不新增前端 framework、runtime dependency 或第二份 authoritative plugin/Tab registry。
- `PanelRegistry`/descriptor projection 必须保持 host projection；不要把 browser UI registry 伪装成 ExtensionManager/Capability authority。
- UI teardown 只能释放 DOM、subscription、RAF、observer、polling、module handles；不能因为 route leave/disable 调用 `pty.close`。
- 保持 `PanelTabDescriptor` 的 API version、trust、permission、status、entry validation；unknown/disabled/incompatible descriptor 必须 fail closed，不得 mount。
- 只在真实测试证明 owner/epoch/lifecycle bug 时修生产代码；不做预防性大重构。
- 中文沟通，代码注释英文，文档中英双语；仓库文档不得写死机器绝对路径。
- 每个 cargo 子进程前从 worktree root 执行 `python qa/browser_live/check_memory.py`，只接受 exit 0；cargo 从所属 package 目录运行并串行执行。
- secrets 不写入文件、报告、trace、截图或静态资产；不启动、停止、替换任何真实 Gateway。

## Required outcome / 必须产出

- 一个 descriptor projection 入口，明确区分 host-owned built-ins 与未来 plugin projection；Terminal descriptor 不再由 `PanelShell` 内部散落构造。
- `PanelShell` 不再依赖 `cfg!(feature = "panel-tab-spike")` 或 `cfg!(feature = "panel-modular")` 来决定 shell identity；active tab、按钮、loading/error/retry 和 mount target 由 descriptor/registry 状态驱动。
- `app.rs` 的模块化路由只负责把适用 route 映射到 host Shell；不能为每个 Tab 增加独立特判分支。Terminal route 的现有行为和 legacy fallback 保持不变。
- Registry/loader 测试证明：排序、duplicate、built-in shadowing、disabled/incompatible/failed 状态、replacement、active removal fallback、unknown route/descriptor fail closed、one active mount 和 cleanup 语义。
- Terminal component fixture 与 production host/wire fixture 继续通过；报告准确说明本阶段只收口通用 host boundary，不新增 live Gateway evidence。
- 更新必要的 `TERMINAL_RUNTIME`、`FEATURE_LOCATOR §6.11`、`SUBSYSTEM_ROUTING` 和一份本阶段交付记录；不重写无关文档。
- 新 worktree clean，提交 conventional commit；不 merge/push。

## Explicitly deferred / 本阶段明确不做

- 不迁移第二个真实 built-in Tab；下一阶段另写 bounded plan 选择具体 Tab。
- 不做 real Gateway smoke；live parity 单独拥有自己的授权和 cleanup gate。
- 不切换默认 `panel-modular`，不删除 legacy Terminal wiring，不恢复/删除 `dist/` 发布资产。
- 不实现 community manifest → install → trust → descriptor projection → update/uninstall lifecycle。
- 不做手机/平板 parity、全量 accessibility/leak matrix、发布回滚和整体 Panel 轻量化宣称。

English handoff: continue from commit `34a3c63bb` in a fresh `panel-shell` worktree. Generalize the host-owned descriptor projection, registry, route selection, and loader lifecycle around the already-accepted Terminal module. Preserve the legacy rollback, keep live Gateway parity separate and blocked unless an authorized disposable target exists, and stop without merge, push, or default-release changes.
