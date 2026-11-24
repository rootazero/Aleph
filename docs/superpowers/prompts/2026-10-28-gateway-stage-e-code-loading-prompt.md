# Aleph Stage E 接续 Prompt — Executable Code Loading Evidence

你是 Aleph Capability Core 的主实现 agent。请在 Aleph 仓库继续工作，以当前 `main` 的 git root 为准。先阅读实际 HEAD 和工作树，不照搬历史 hash。会话交接时 `main` 为 `ee6e65885`，Stage C/D 已合并且开发 worktree 已清理。

## 架构终极目标

Everything externally composable is a Capability.

把 Aleph 从“一个集成很多功能的进程”演进为轻量 Capability Core 与按需启动、停止、替换的能力提供者。关闭 Gateway 通道、终端、记忆星云图或 sandbox 后，不应初始化其线程、连接、数据库或缓存；单个能力失败不应拖垮 Core；旧调用句柄必须失效。下一步还要证明 provider 的 guest bytes 在 disabled/cold 时不被读入或实例化，并在 dispose 释放最后一个 `Arc<LoadedWasmPlugin>` 后观察 exact guest instance drop；这不等于 engine/JIT/OS mapping/RSS 已回收。

## 开始前重述边界

请先用你自己的话重述：Stage D 已证明两种 test composition 的运行时资源 cold/active/replacement/failure 边界，但没有证明所有内置通道的静态代码被卸载，也没有证明 RSS 下降。本任务是 Stage E 的一个 bounded Extism 纵向切片：为一个真实 test guest 建立“cold 不读取/不实例化 guest code，activation 读取并执行，stop/replacement 使旧 binding 失效，并在释放所有 owner Arc 后释放 exact loaded guest instance”的可执行证据。不得把 Extism `Plugin` drop 写成整个引擎 JIT code、所有 anonymous pages 或物理 RAM 已回收。

本任务是 Stage E bounded code-instance evidence，不是 native compiled-code unloading。它只扩展现有 Extism runtime 的 testable boundary：guest bytes、loaded instance 和 existing unmount/dispose 的 exact ownership。

## 必读材料

- `AGENTS.md`
- `docs/superpowers/plans/2026-10-28-gateway-stage-e-code-loading-plan.md`
- `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` §6–7
- `docs/superpowers/runs/2026-10-28-gateway-stage-d-resource-matrices-run.md`
- `docs/reference/PLUGIN_SYSTEM.md`
- `docs/research/aleph_modularization_plan.md` §0.4（注意其 2026-10-22 接线实况可能已过时，以当前代码为准）
- `docs/reference/FEATURE_LOCATOR.md` §3.5d、§3.5h
- `src/extension/plugin_state.rs`
- `src/extension/lifecycle.rs`
- `src/extension/loader.rs`
- `src/extension/runtime_owner.rs`
- `src/extension/runtime/wasm/mod.rs`
- `src/extension/runtime/wasm/tests/guest_fixture.rs`
- `src/capability/provider.rs`

## 当前代码事实（必须复核）

- durable enabled/disabled 与 mount/unmount 仍由 `plugin_state.rs` + `ExtensionManager` 所有；不得新增第二份判定。
- `PluginLoader` 的 `WasmRuntime` 是 lazy `Option`；cold discovery 不应构造它。
- `WasmRuntime` 以 `HashMap<String, Arc<LoadedWasmPlugin>>` 持有实际 Extism instance；`WasmBinding` 和 `WasmMountIdentity` 也持有 `Arc`，因此 map removal 不等于 exact instance 已释放。
- cold case 没有 `WasmRuntime` 可供计数；cold/activation read receipt 必须由 `PluginLoader` 的 test-only probe 持有，并在创建 `WasmRuntime` 时传入。instance create/drop receipt 由 exact `LoadedWasmPlugin` 持有。
- 现有 stop/admission/generation/identity/dispose 路径是权威；不得新增 deferred-cleanup authority 或 detached cleanup。
- 现有 guest fixture 会生成真实 WASM bytes，可复用，不需要引入 Wasmtime、libloading 或新 runtime。
- `src/extension/lifecycle.rs` 的 `b3_*` 测试已经覆盖 activation、stop、replacement 的行为；本 slice 扩展这些测试的 receipts，不重复声称这些行为是新保证。
- 当前 activation 通过 `Wasm::file` + `PluginBuilder::build` 加载 guest；extism 的 `Wasm::file` 读取发生在 builder 内部，无法在现有边界观测。因此本 slice 若需要真实 guest-byte receipt，固定改为 `std::fs::read` + `Wasm::data`，接受 bytes 与编译产物短暂共存的峰值内存取舍。
- `unload_plugin`、`remove_if_current` 以及 `PluginLoader::shutdown` 都是 instance-drop census 的路径；shutdown 的全 runtime drop 不得被误报为 identity-fenced replacement cleanup。

## 执行要求

1. 确认 `main` 干净，从当前 `main` 创建短分支 `stage-e-code-loading`；所有生产 writer 串行。
2. 先完成 loader/instance ownership census 和 cold/activation/dispose baseline，再写生产代码。区分五种证据：manifest reads、guest-byte reads、Extism instance create/drop、OS mappings、RSS。binary size 或 RSS 单独都不能证明 code unloaded。
3. 先写行为 RED：cold discovery/list/descriptor/projection 不读取 guest bytes、不创建 runtime/instance；activation positive control 必须读取真实 bytes、创建 instance 并执行 guest；stop/replacement 必须拒绝旧 binding，并在 `unmount().await` 返回、所有测试保留的 `Arc`/binding/identity/receipt clone 都释放后观察 exact old instance drop；failed build/activation 不得伪造 created/dropped/clean。扩展现有 `src/extension/lifecycle.rs` `b3_*` activation/stop/replacement tests，不把已 GREEN 的行为重新包装成五个新保证。
4. 在现有 `PluginLoader` loading boundary 显式 `std::fs::read` guest bytes，再交给 `Wasm::data` 和 Extism builder；receipt 是观测，不是第二份 lifecycle authority。instance create 只在 `PluginBuilder::build` 成功后记录；instance drop 只在 exact `LoadedWasmPlugin` 最后一个 owner 释放后观察。
5. stop/replacement 必须沿用现有 admission fence、operation drain、identity/generation check 和 disposer。只有 exact old instance 的所有 owner/binding/identity 释放后，才能声称 provider-owned instance 已释放；失败必须保留 residual。
6. 不得新增 unsafe、native `dlopen`/`dlclose`、全局隐式 loader、第二 registry/process model、新依赖或 production mount。若必须修改公开 plugin lifecycle、引入全局 engine/cache、采用进程隔离或 native unloading，停下来请求架构决策。
7. 每条 Cargo 命令前单独运行并记录：

   ```bash
   python qa/browser_live/check_memory.py
   ```

   要求 exit 0 / `ALLOW`。零测试 filter 不算通过。
8. focused tests 连续串行运行三次；执行完整 Extension/WASM、Stage D matrix 和 Stage C regression set，以及 check/clippy/fmt/diff。若 feature/build 表面扩大，增加对应 feature 与 workspace 检查。
9. 对完整 diff 做独立只读 review，重点检查 cold read 是否隐式加载、旧 Arc/binding 是否残留、partial activation 是否泄露、drop receipt 是否被误写为 OS unload、是否新增第二 authority。reviewer 不可用时记录完整错误并手工执行同一 checklist，不得声称独立通过。
10. 只有全部 GREEN 且无未解决 Critical/Important，才更新双语 run record、源计划、`docs/reference/GATEWAY.md`、`docs/reference/PLUGIN_SYSTEM.md` 和 `FEATURE_LOCATOR.md` §3.5i。无法证明的 JIT/Engine/RSS/OS-page 边界必须明确保留为 unproven。

## 最终报告

列出 baseline、root cause/evidence gap、方案取舍、RED/GREEN 命令与真实结果、修改文件、guest-byte/instance/OS/RSS 各自证明了什么、authority duplication 审查、review 可用性、首个未闭合 contract、仍 deferred 的 provider/kind、未运行验证，以及没有执行的 destructive/external action。

本 bounded Stage E 闭合仍不代表所有内置 Gateway 通道已动态加载，也不自动授权下一阶段。
