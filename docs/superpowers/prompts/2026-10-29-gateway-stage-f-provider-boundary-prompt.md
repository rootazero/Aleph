# Aleph Stage F 接续 Prompt — Provider-Boundary Code Lifecycle Evidence

你是 Aleph Capability Core 的主实现 agent。请在 Aleph 仓库继续工作，以当前 `main` 的 git root 为准。先阅读实际 HEAD、工作树和本 prompt/plan，不照搬历史 hash。

当前 `main` 已包含 Stage E：`07250ba1f`（代码与文档），其代码提交为 `7c31a5fa6`。Stage E 只证明了既有 `PluginLoader`/`ExtensionManager` 内，一个真实 Extism test guest 的 guest-byte read、instance create、A/B replacement、stale binding rejection 和 exact `LoadedWasmPlugin` drop。它没有证明 provider-facing capability boundary 已经承载这份 code lifecycle，也没有授权 production Zahir mount、Feishu/R2 接线或所有内置 channel 动态加载。

## 架构目标与当前边界

Everything externally composable is a Capability。

本阶段要把 Stage E 的 bounded Extism 证据推进到现有 `ProviderHandle` / `RuntimeOwner` / `ExtensionManager` 的 provider boundary：disabled/discovered/cold 时不读取 guest bytes、不创建 runtime/instance；provider activation 进入现有 Starting/Ready 与 generation/admission 路径后，读取并执行真实 guest；stop/replacement 让旧 generation、旧 binding 和旧 provider call 失效；释放所有 retained owner/binding/identity `Arc` 后，观察 exact old `LoadedWasmPlugin` drop；失败 activation 保留真实 residual，不伪造 clean/unloaded。

这是 provider-boundary evidence，不是 native compiled-code unloading。Extism `Plugin` drop 不等于 engine-wide JIT、anonymous mappings、OS executable pages、线程或 RSS 已回收。

## 必读材料

- `AGENTS.md`
- `docs/reference/REDLINES.md`
- `docs/reference/FEATURE_LOCATOR.md` §3.5d、§3.5h、§3.5i
- `docs/reference/PLUGIN_SYSTEM.md`
- `docs/reference/GATEWAY.md`
- `docs/superpowers/plans/2026-10-29-gateway-stage-f-provider-boundary-plan.md`
- `docs/superpowers/plans/2026-10-28-gateway-stage-e-code-loading-plan.md`
- `docs/superpowers/runs/2026-10-28-gateway-stage-e-code-loading-run.md`
- `src/capability/provider.rs`
- `src/capability/provider.rs` 的现有 generation/admission 测试
- `src/extension/plugin_state.rs`
- `src/extension/lifecycle.rs`
- `src/extension/runtime_owner.rs`
- `src/extension/loader.rs`
- `src/extension/runtime/wasm/mod.rs`
- `src/extension/runtime/wasm/tests/guest_fixture.rs`
- `src/capability/zahir_facade.rs`

## 开始前必须重述

请先用自己的话说明：Stage E 已证明 Extension-owned Extism instance boundary，但第一条未闭合 contract 是 provider state/generation/admission 与 guest instance ownership 是否由同一条现有 lifecycle 路径约束。本阶段只补这个 bounded provider composition 证据，不把 test composition 宣称为 production mount，也不把 capability provider 的状态 receipt 当成第二套 lifecycle authority。

## 当前事实必须复核

- `plugin_state.rs` 与 `ExtensionManager` 仍拥有 durable enabled/disabled、discover、mount/unmount authority。
- `ProviderHandle` / `ProviderLifecycle` 拥有 provider state、generation、call admission 与 transition notification；不要新增平行 owner map、generation map 或 cleanup registry。
- `RuntimeOwner` 是现有 provider/runtime ownership 组合；先确认它是否已经携带足够的 provider identity、generation 和 disposer，再决定是否需要最小 test seam。
- `PluginLoader` 的 WASM runtime 仍是 lazy `Option`；Stage E 的 loader-owned test probe 与 exact `LoadedWasmPlugin` receipt 应继续复用。
- `WasmBinding`、`WasmMountIdentity`、provider generation 与现有 disposer 是 stale-call 和 replacement 的权威判据。
- `ZahirFacade::new` 目前没有 production mount；本阶段不得自行挂载它。
- 若需要修改公开 plugin lifecycle、引入全局 engine/cache、进程隔离、native unloading、第二 registry 或 production mount，停止并请求架构决策。

## 执行规则

1. 确认 `main` 干净并从实际 `main` 创建不超过三个单词的短分支，例如 `stage-f-provider-boundary`。所有生产 writer 串行。
2. 先做 provider/runtime ownership census 和 baseline，记录现有 ProviderState、generation、admission、RuntimeOwner、unmount/dispose 的实际调用线。不要复制 Stage E 的历史结论而不复核当前代码。
3. 先写行为 RED，且必须作用在现有 provider composition 上：
   - disabled/discovered/cold provider list/descriptor/projection 产生 zero guest-byte read、zero runtime/instance；
   - activation 走既有 provider transition 与 generation，执行真实 guest payload A；
   - provider call admission 只接受 Ready + current generation；Stopping/Stopped/Disabled/Failed 和旧 generation 均拒绝；
   - replacement 走现有 unmount/dispose → new activation 路径，payload B 可执行，旧 binding/provider handle 失效；
   - 所有 retained owner/binding/identity `Arc` 释放后，旧 exact `LoadedWasmPlugin` 才能被观察为 dropped；
   - failed activation/read/build 不得伪造 created/dropped/clean，失败 residual 必须保留在现有错误语义中。
4. 只复用 Stage E 的 loader probe/receipt；不把 provider transition counter、capability projection 或 broadcast notification 做成第二份 guest lifecycle authority。每个 receipt 必须能回到真实 `LoadedWasmPlugin` incarnation。
5. 不把 `ProviderState` 的变化等同于 guest bytes/instance 的变化：分别记录 manifest read、guest-byte read、instance create/drop、provider state/generation、OS mapping、RSS。
6. 保持代码注释英文，新增文档中英双语。每条 Cargo 命令前单独运行并记录：

   ```bash
   python qa/browser_live/check_memory.py
   ```

   必须得到 exit 0 / `ALLOW`。零测试 filter 不算通过。
7. focused provider-boundary tests 连续串行运行三次；再运行相关 Extension/WASM、Provider/Capability、B3、Stage C/D 窄回归、check/clippy/fmt/diff。若 feature、slot API、workspace 或 bin surface 变化，增加相应检查。
8. 完整 diff 交给独立只读 reviewer，重点检查 provider state/generation 是否与既有 authority 同源、是否产生第二 lifecycle authority、旧 Arc 是否残留、失败是否伪造 clean、ProviderState 是否被错误当成 OS unload。Critical/Important 必须修复并重新 review；reviewer 不可用时保存完整错误并手工执行 checklist，不得宣称独立通过。
9. 只有 GREEN 且 review 无未解决 Critical/Important 后，才更新双语 run record、Stage F plan、`FEATURE_LOCATOR.md` 新条目及必要的 reference docs。

## 禁止事项

- 不新增 `unsafe`、`dlopen`/`dlclose`、`libloading`、Wasmtime、新 runtime、新 dependency。
- 不新增全局隐式 loader、第二 registry、第二 generation/owner map、detached cleanup 或 deferred cleanup authority。
- 不修改生产 Zahir mount，不接 Feishu/R2/Terminal/Memory/Sandbox，不把单个 test guest 证据扩大成所有内置 channel 动态加载。
- 不以 binary size、OS mapping 或 RSS 单独证明 code unloaded。
- 不把 ProviderState、generation、broadcast notification 或 projection snapshot 当作 exact Extism instance drop 的替代证据。

## 最终报告必须回答

1. provider disabled/discovered/cold、activation、replacement 各自读了什么、创建了什么？
2. 哪个现有 authority 产生 state/generation/admission 判据？是否存在第二份 lifecycle authority？
3. 哪些旧 provider/binding/identity/owner `Arc` 必须释放后才能观察 exact instance drop？
4. failed activation 的 residual 是什么？
5. OS mapping、engine/JIT、线程和 RSS 哪些仍然无法证明？
6. 是否有任何 production channel 或 Zahir mount 被接入？预期答案：没有。
7. 实际运行了哪些命令、reviewer 是否可用、哪些 provider/kind/production integration 仍 deferred？
8. 没有执行哪些 destructive/external action？
