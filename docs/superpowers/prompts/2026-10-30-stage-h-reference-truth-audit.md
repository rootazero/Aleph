# Stage H Reference Truth Audit Prompt
# Stage H Reference Truth Audit 执行提示

你在 `/Volumes/TBU4/Workspace/Aleph` 继续工作。当前 Stage H bounded implementation 已提交到 `main`：

- HEAD: `e759b3601a9e6ee50c96dd283fcf105b71a85c24`
- 提交: `feat(extension): separate discovery from activation`
- 当前实现已通过 Stage H focused tests 三次、Extension lifecycle 76 项、Capability 222 项、Gateway plugin handlers 15 项和 `cargo check -p alephcore --tests`。
- 全仓 clippy 与 fmt 仍有未修改文件的既有基线错误；它们不属于本次文档校对任务。

## Mission / 任务

完成 Stage H 之后的 source-backed reference 文档校对，使文档准确表达当前生产事实与 North Star 边界：

> Everything externally composable is a Capability.

本次只校对文档，不新增 provider 接线，不修改 production Rust，不接入 Feishu、Terminal、Memory、Sandbox 或其他 capability kind。

## 已确认的事实源

优先以以下代码为事实源：

- `src/extension/mod.rs::ExtensionManager::ensure_discovered`
- `src/extension/mod.rs::ExtensionManager::ensure_loaded`
- `src/extension/lifecycle.rs::DiscoveryMode`
- `src/extension/runtime_owner.rs`
- `src/capability/provider.rs`
- `src/tools/registry.rs`
- `src/capability/projection_host.rs`
- `src/capability/zahir_facade.rs`
- `src/gateway/handlers/plugins/handlers/manage.rs`
- `src/builtin_tools/plugin_manage.rs`
- `src/hub/reconcile.rs`

必须保持的边界：

- `ExtensionManager` / `RuntimeOwner` 是 tool-only WASM bounded slice 的 activation owner。
- `ensure_discovered()` 只做 declaration discovery，不能启动 provider、读取 guest bytes、创建 guest instance、发布 callable surface。
- `ensure_loaded()`、`set_plugin_enabled()` 和既有 owner control path 才能触发显式 activation。
- `ToolHandlerRegistry` 是 canonical callable publication source。
- `ProjectionHost` / `ZahirFacade` 是 derived read/projection/notification surface，不拥有 provider lifecycle。
- Feishu、其他 Gateway channel、Terminal、Memory、Sandbox、EffectClaim producer、Safe Replay、universal scheduler 和其余 capability kinds 仍 deferred。
- 不得声称证明 native unload、JIT/engine reclamation、OS mapping reclamation、thread shutdown、RSS reduction 或 process isolation。

## 必查的已知陈旧表述

重点核对并修复：

1. `docs/reference/FEATURE_LOCATOR.md` §3.5d / 2026-10-22 段落仍把 lazy activation 写成“只做设计 + 入口、不接线”，并声称 production 没有 provider transition caller、`new_with_provider` 没有 production caller。该表述必须改为当前事实，同时保留 `new_with_provider` 不是 plugin lifecycle authority 的边界。
2. `FEATURE_LOCATOR.md` B6 段落必须区分：
   - declaration-only read discovery 已由 Stage H 实现；
   - startup catalog、agent run 和 explicit control 仍是 activation boundaries；
   - `describe` / `resolve` / `project` / ProjectionHost read-side 不启动 provider。
3. `ARCHITECTURE.md` 中的历史 B5/B6 记录只有在日期与历史范围明确时才能保留；如文字会被读作当前 wiring reality，补充当前 Stage H 交叉说明。
4. `PLUGIN_SYSTEM.md` 的中英文段落必须与上述事实完全一致。
5. 搜索其他 reference 文档中相同的旧事实，不要只修已知行号；每个修改都要能指向当前 source anchor 或明确的历史记录。

## 工作纪律

- 先验证 root、branch、HEAD、worktree、remote relationship 和工作区状态。
- 读取 `AGENTS.md` 以及相关 reference 文档后再修改。
- 不修改 `AGENTS.md`、`CLAUDE.md`、production Rust 或测试代码。
- 不运行 `cargo fmt` 覆盖全仓，不修复无关 clippy/fmt 基线；如发现需要代码改变才能使文档成立，停止并记录 follow-up。
- 文档新增或修改保持中英双语；保留日期、证据范围和 deferred 限制。
- 不执行 commit、merge、push 或发布，除非当前会话另有明确授权。

## 交付物

1. 修正后的 `FEATURE_LOCATOR.md`、`ARCHITECTURE.md`、`PLUGIN_SYSTEM.md` 及必要的其他 reference 文档。
2. 一份 stale-claim census：列出搜索到的旧表述、处理方式和 source anchor。
3. `git diff --check`、双语段落一致性检查和文档链接检查结果。
4. 最终报告明确写出未修改的 production 行为、未运行的 Cargo 检查、仍 deferred 的 provider/kind，以及没有执行的外部或 destructive 操作。

先做只读审计和事实矩阵，再修改文档；不要直接大范围重写 FEATURE_LOCATOR。