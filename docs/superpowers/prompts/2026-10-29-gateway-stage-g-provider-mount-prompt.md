# Aleph Stage G 接续 Prompt — Provider-Neutral Production Mount Readiness

你是 Aleph Capability Core 的主实现 agent。请在 Aleph 仓库继续工作，以当前 `main` 的 git root 为准。先读取实际 HEAD、工作树、本 prompt 和 plan；不要把 prompt 中的历史 hash 当作当前事实。

## 当前进度

Stage E 已证明既有 `PluginLoader` / `ExtensionManager` 内真实 Extism guest 的 guest-byte read、instance create、A/B replacement、stale binding rejection 和 exact `LoadedWasmPlugin` drop。

Stage F 已把这份 bounded evidence 推进到现有 provider boundary，并已合并到 `main`。它证明了：

- discovered、Cold、Disabled 的 provider list/status/descriptor/projection 不读取 guest bytes，也不创建 runtime 或 instance；
- 真实生产 enable path 经 `ExtensionManager::set_plugin_enabled` 进入 `ProviderHandle` 的 Ready/current-generation admission，并执行 payload A；
- replacement 经现有 reload/unmount/dispose → new activation 路径执行 payload B，旧 provider/canonical/binding identity 失效；
- 释放旧 owner、canonical lease、binding 和 publication identity 后，旧 exact `LoadedWasmPlugin` receipt 才 drop；successor receipt 与 `[old, new]` drop 顺序均已验证；
- failed read、failed build、publication collision 和 disposer failure 保留真实 residual，不伪造 clean Ready/create/drop。

Stage F 的最终代码提交是 `b8f3776d9`，合并提交是 `52942ada1`；这些只用于定位历史，不替代当前 HEAD 检查。

## 终极目标

Everything externally composable is a Capability。

一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability。Aleph 应从“一个集成很多功能的进程”演进为“一个轻量 Capability Core，加上一组按需启动、可停止、可替换的能力提供者”，覆盖 Gateway 社交通道、Terminal、Memory Graph、Sandbox 等 provider。

## 本阶段目标

本阶段不是直接接 Feishu、R2、Terminal、Memory 或 Sandbox，也不是把单个 test guest 证据扩大成所有 provider 动态加载。

本阶段要完成 provider-neutral production mount readiness，并在架构边界允许时实现最小 production `ZahirFacade` mount：

1. 复核生产代码中 canonical `ToolHandlerRegistry`、`OwnershipTree`、`ProviderHandle`、`RuntimeOwner` 和 ExtensionManager 的真实 owner 与生命周期；
2. 判断现有 `ZahirFacade` 是否可以在一个已有 host composition point 被挂载，而不新增 registry、owner map、generation map、descriptor catalog、runtime table 或 cleanup authority；
3. 先写 design/RED，证明 provider-neutral facade mount 与现有 lifecycle 同源；
4. 只有在确认 production mount 可以复用现有 authority 后，才实现最小 mount；如果需要新 owner、第二套 lifecycle、跨 channel registry 或改变公开 plugin lifecycle，停止并请求架构决策；
5. mount 只验证 provider-neutral capability host，不接任何具体生产 channel。

第一条必须重述的 contract：Stage F 证明了 provider state/generation/admission 与 guest instance ownership 由同一条 Extension lifecycle 路径约束；Stage G 只解决这条已验证路径能否被一个 provider-neutral production host 挂载，不能把 facade projection、provider state 或 broadcast notification 变成第二套 lifecycle authority。

## 必读材料

- `AGENTS.md`
- `docs/reference/REDLINES.md`
- `docs/reference/FEATURE_LOCATOR.md` §3.5d、§3.5h、§3.5i、§3.5j
- `docs/reference/PLUGIN_SYSTEM.md`
- `docs/reference/GATEWAY.md`
- `docs/superpowers/plans/2026-10-29-gateway-stage-f-provider-boundary-plan.md`
- `docs/superpowers/runs/2026-10-29-gateway-stage-f-provider-boundary-run.md`
- `src/capability/zahir_facade.rs`
- `src/capability/backend.rs`
- `src/capability/ownership.rs`
- `src/capability/provider.rs`
- `src/extension/plugin_state.rs`
- `src/extension/lifecycle.rs`
- `src/extension/runtime_owner.rs`
- `src/extension/loader.rs`
- `src/extension/runtime/wasm/mod.rs`
- `src/gateway/channel_registry.rs`

## 架构红线

- `plugin_state.rs` / `ExtensionManager` 仍是 durable enabled/disabled、discovery、mount/unmount authority。
- `ProviderHandle` / `ProviderLifecycle` 仍是 provider state、per-handle generation、call admission 和 transition authority。
- `RuntimeOwner` 仍是 provider/runtime ownership 组合，负责 owner generation、identity、receipt、admission、task/disposer。
- `ToolHandlerRegistry` 是 canonical handler source；`OwnershipTree` 是 owner/revoke/dispose source。不能复制它们。
- `ZahirFacade` 只能组合既有 registry/tree；不能自行成为第二个 registry、owner map、generation map 或 cleanup authority。
- 没有 architecture approval 时，不得新增 production Zahir mount；本阶段首先必须完成 readiness/design gate。
- 不新增 `unsafe`、`dlopen`/`dlclose`、`libloading`、Wasmtime、新 runtime、新 dependency、global loader/cache、process isolation、native unloading 或 detached cleanup。
- 不接 Feishu、R2、Terminal、Memory、Sandbox 或所有内置 channel 的动态加载。
- 不用 ProviderState、generation、projection snapshot 或 broadcast notification 替代 exact Extism receipt/drop 证据。

## 开始工作时必须完成

1. 检查实际 `main`、工作树和 worktree；创建不超过三个单词的短分支，例如 `stage-g-provider-mount`。
2. 做 production ownership census，明确：
   - 生产 `ToolHandlerRegistry` 的创建与生命周期；
   - 生产 `OwnershipTree` 是否已有 owner；
   - `ZahirFacade` 的唯一可挂载 host；
   - ExtensionManager、Gateway、Capability projection host 的依赖方向；
   - mount、stop、replacement、dispose 的调用线。
3. 如果生产 `OwnershipTree` 不存在，或 facade mount 必须创建第二个 owner/lifecycle authority，不要偷偷补一个。保存 census，停止实现，向用户请求架构决策。
4. 如果现有 authority 足够，先写 provider-neutral RED，再写 design/plan；不要先写 channel adapter。

## 预期行为证据

若 readiness gate 通过，测试必须作用于真实现有 composition：

- host 未挂载或 provider Cold 时，descriptor/list/project 产生 zero guest-byte read、zero runtime/instance；
- mount 只暴露现有 canonical registry/tree 的 facade，不创建第二份 descriptor/handler/owner/generation state；
- 通过现有 ExtensionManager activation 执行真实 fixture payload A，facade-facing call 与 canonical call 使用同一个 provider admission；
- replacement 通过既有 unmount/dispose → activation 执行 payload B，旧 facade/provider/binding identity fail closed；
- 释放 retained facade、owner、binding、lease、identity Arc 后，exact old receipt 才 drop，不能只看 ProviderState；
- failed read/build/publication/dispose 保留现有 residual，不伪造 clean mount 或 unload；
- facade mount/unmount 本身不得改变 guest byte、instance、OS mapping、thread 或 RSS 的证据含义。

如果无法在不新增 authority 的情况下满足这些行为，Stage G 的正确结果是 NO-GO/deferred，并产出具体架构决策请求，而不是扩大实现范围。

## 验证纪律

- 每条 Cargo 命令前单独执行 `python qa/browser_live/check_memory.py`，必须 exit 0 / `ALLOW`；
- focused host/provider tests 连续串行三次；
- 再运行相关 WASM、Stage E/F、B3、capability、gateway narrow regressions、check、clippy、fmt、diff；
- 零测试 filter 不算证据；
- 新文档中英双语，代码注释英文；
- 完整 diff 交给独立只读 reviewer，重点检查是否新增第二 authority、是否错误拥有 OwnershipTree、mount 与 Extension lifecycle 是否同源、旧 Arc 是否残留、是否把 facade 状态当作 unload；
- reviewer 不可用时保存完整错误并手工执行 checklist，不得宣称独立通过。

## 最终报告必须回答

1. production mount 复用了哪些既有 registry/tree/provider/runtime owner？
2. mount、activation、replacement、stop 各自读了什么、创建了什么？
3. provider state/generation/admission 的唯一 authority 是什么？有没有第二套 lifecycle authority？
4. 哪些 facade/provider/binding/lease/identity/owner Arc 必须释放后才能观察 exact instance drop？
5. failed mount/activation 的 residual 是什么？
6. OS mapping、engine/JIT、线程和 RSS 哪些仍无法证明？
7. 是否接入任何生产 channel？预期答案仍是没有，除非用户明确批准具体 channel。
8. 如果 readiness gate NO-GO，准确指出缺失的生产 owner、架构决策和下一步选择。
9. 实际运行了哪些命令、reviewer 是否可用、哪些 provider/kind/integration 仍 deferred？
10. 没有执行哪些 destructive/external action？
