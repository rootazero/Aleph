# Stage H Reference 文档定点校对设计
# Stage H Reference Documentation Point Correction Design

- 状态 / Status: Proposed; awaiting explicit approval
- 日期 / Date: 2026-10-30
- 当前 checkout / Current checkout: `main` at `be9c68c46e7253b815d0ff8e175a803e9dbfb5f7`, synchronized with `origin/main`

## Intent / 意图

本次校对使 reference 文档准确表达 Stage H 之后的生产事实，并保持 North Star：**Everything externally composable is a Capability.** 重点是让读者能够区分 declaration-only discovery、显式 provider activation，以及 derived projection/read surfaces；同时不把 Tool-only WASM bounded slice 扩大成 Feishu、其他 Gateway channel 或其他 capability kind 的生产接线。

This correction aligns the reference documentation with the post-Stage-H production facts while preserving the North Star: **Everything externally composable is a Capability.** It makes the boundary between declaration-only discovery, explicit provider activation, and derived projection/read surfaces explicit without expanding the bounded Tool-only WASM slice into Feishu, other Gateway channels, or other capability kinds.

## Goals / 目标

- 修正 `docs/reference/FEATURE_LOCATOR.md` §3.5d 中把 lazy activation 描述为“只做设计 + 入口、不接线”的陈旧 current-looking 表述，并补充当前 bounded wiring。
- 强化 `FEATURE_LOCATOR.md` B6：`ensure_discovered()` 是 declaration-only read boundary；`ensure_loaded()`、boot catalog/agent run 和 explicit enable/reload/control 是 activation boundaries；`describe`、`resolve`、`project`、ProjectionHost read-side 不启动 provider。
- 将 `FEATURE_LOCATOR.md` Stage F 中“没有 production `ZahirFacade::new` mount”的句子明确限定为当时的历史范围，并交叉说明当前 Stage G Tool host mount 与 Feishu/channel deferred 状态。
- 校准 `ARCHITECTURE.md` 的 B6 historical wording 与 Provider lifecycle foundation，避免把历史 B7/Feishu scope 读成当前 Feishu Capability wiring。
- 校准 `PLUGIN_SYSTEM.md` 2026-10-21 Provider foundation 的历史表述，使其与当前 `ExtensionManager`/`RuntimeOwner` 驱动 bounded tool-only WASM transition 的段落一致。
- 保留 `ToolHandlerRegistry` 为 canonical callable publication source；保留 `ProjectionHost` / `ZahirFacade` 为 derived read/projection/notification/host-local worker surface，不赋予其 provider lifecycle ownership。
- 交付 stale-claim census，说明搜索到的旧表述、处理方式及 source anchor。

## Non-goals / 非目标

- 不修改 production Rust、测试、`AGENTS.md` 或 `CLAUDE.md`。
- 不新增 provider 接线，不接 Feishu、其他 Gateway channel、Terminal、Memory、Sandbox 或其余 capability kinds。
- 不实现 EffectClaim producer、Safe Replay、universal durable scheduler、native unload、JIT/engine reclamation、OS mapping/executable-page reclamation、thread shutdown、RSS reduction 或 process isolation。
- 不大范围重写带日期的历史记录；只修改会被读作当前事实的陈旧表述，或为其增加明确的日期、范围和 current/deferred cross-reference。
- 不执行 Cargo 全仓 fmt/clippy 修复，不 commit、merge、push 或发布。

## Constraints / 约束

- 文档新增或修改保持中英双语。
- 事实优先来自当前 checkout 的 source anchors：
  - `src/extension/mod.rs:698` — `pub async fn ensure_discovered(&self) -> ExtensionResult<()>`
  - `src/extension/mod.rs:738` — `pub async fn ensure_loaded(&self) -> ExtensionResult<()>`
  - `src/extension/lifecycle.rs:146-149` — `DiscoveryMode::{DeclarationsOnly, Activation, FullLoad}`
  - `src/extension/lifecycle.rs:357-360`, `:379-382`, `:546-550` — declaration discovery、FullLoad cleanup、Cold declaration without mount
  - `src/extension/runtime_owner.rs` — `RuntimeOwner` owns bounded Tool-only WASM activation; no callable registry or durable intent
  - `src/tools/registry.rs:999`, `:1009` — `new()` and `new_with_provider(provider: ProviderHandle)`; the latter remains a constructor/assembly seam, not plugin lifecycle authority
  - `src/bin/aleph-server/commands/start/mod.rs:269-275` — production ProjectionHost mount/install/wait
  - `src/capability/projection_host.rs:76,532,540,725` and `src/capability/zahir_facade.rs` — derived host/read/projection/notification surfaces
  - `src/gateway/handlers/plugins/handlers/manage.rs`, `src/builtin_tools/plugin_manage.rs`, `src/hub/reconcile.rs` — declaration-only read callers and explicit control boundaries
- Current checkout is authoritative if it differs from the user-supplied historical Stage H commit hash.

## Approaches considered / 已考虑方案

### A. 定点纠偏 / Point correction — selected

只修改 `FEATURE_LOCATOR.md` 的 §3.5d、B6、Stage F historical sentence，以及 `ARCHITECTURE.md` / `PLUGIN_SYSTEM.md` 的 Provider foundation 与 B6 historical cross-reference；其余 reference 文件纳入 census 但不改。

This keeps the diff small and the evidence chain reviewable. The trade-off is that unrelated historical records remain unchanged and must be interpreted through their dates and the census.

### B. 统一 current-status 侧栏 / Shared current-status sidebar

在三个主文档中复制一段完整的 current/deferred status，再给历史段落加链接。它能降低误读风险，但会复制双语事实并增加后续同步成本。

### C. 全量逐条校准 / Broad line-by-line calibration

逐条重写 17 个 reference 文件中的所有旧 claim。它的全局一致性更高，但 diff 大、历史证据风险高，且违反本次避免大范围重写的纪律。

## Chosen design / 选定设计

采用 A。每个修改点都保留原有历史日期和证据范围，并在必要处加一句当前状态：Stage H 已使 bounded `ExtensionManager` / `RuntimeOwner` tool-only WASM activation 成为生产事实；`ensure_discovered()` 仍只做 declaration discovery；`ToolHandlerRegistry` 是 callable publication authority；`ProjectionHost` / `ZahirFacade` 不拥有 provider lifecycle；Feishu、其他 Gateway channel、Terminal、Memory、Sandbox、EffectClaim producer、Safe Replay、universal scheduler 和其余 capability kinds 仍 deferred。

Use A. Each edit will preserve the original historical date and evidence scope and add a current-status sentence where needed: Stage H makes bounded `ExtensionManager` / `RuntimeOwner` Tool-only WASM activation a production fact; `ensure_discovered()` remains declaration-only; `ToolHandlerRegistry` is the callable publication authority; `ProjectionHost` / `ZahirFacade` do not own provider lifecycle; Feishu, other Gateway channels, Terminal, Memory, Sandbox, EffectClaim producer, Safe Replay, the universal scheduler, and remaining capability kinds remain deferred.

## Planned document edits / 计划修改

1. `FEATURE_LOCATOR.md` §3.5d：将 2026-10-22 tri-state wiring paragraph 标记为 pre-Stage-H historical snapshot，移除其 current-looking conclusion，补充 current bounded wiring and `new_with_provider` boundary。
2. `FEATURE_LOCATOR.md` B6：补充 discovery-only/read-side/activation boundary wording；给 B6→B7/Feishu sentence 加历史限定，并指向 B8.2 deferred。
3. `FEATURE_LOCATOR.md` Stage F evidence：将 `ZahirFacade::new` 缺失描述限定为 Stage F historical scope，保留 Feishu/channel deferred。
4. `ARCHITECTURE.md`：澄清 Provider foundation 的历史与当前关系；给 B6 historical Feishu sentence 加范围限定和当前 deferred cross-reference。
5. `PLUGIN_SYSTEM.md`：重写 2026-10-21 Provider foundation 的英中段落，使历史 baseline 与 current bounded production slice 不冲突；保留 `new_with_provider` 的 compatibility-constructor boundary。
6. 生成 stale-claim census：列出改写或保留的旧 claim、处理理由、双语对应段落和 source anchor。

## Acceptance checks / 验收检查

- `git diff --check` 通过。
- 双语段落逐项对照：英文与中文表达相同的 scope、日期、activation/read boundary 和 deferred limitations。
- 文档内部链接与相对路径链接可解析；新增交叉引用指向存在的 heading/file。
- `rg` 复查旧 current-looking claims：陈旧句已删除或明确标记 historical scope；保留的历史记录列入 census。
- 不修改 production Rust/test/AGENTS/CLAUDE；不运行未授权的 Cargo 全仓检查；最终报告明确列出这些未执行项与仍 deferred 的 capability/provider。

## Builder check resolution / Builder check 处理

Builder reviewer identified one approval gate and two implementation details. The approval gate remains explicit: this design must be approved before implementation planning. The implementation details are settled here: the stale-claim census will be a dedicated section of the final delivery report, and it will cover each changed or intentionally retained current-looking claim; the report will also state that the remaining broad-search hits are dated historical records left unchanged. These choices do not expand the selected point-correction scope.

Builder reviewer 识别出一个审批门槛和两个实施细节。审批门槛保持不变：本设计必须先获得明确批准，之后才能进入实施计划。两个实施细节在此确定：stale-claim census 作为最终交付报告中的独立章节，覆盖每个被修改或有意保留的 current-looking claim；报告同时说明其余宽搜索命中属于带日期的历史记录，保持不变。这些决定不扩大已选定的定点纠偏范围。

## Builder decisions / 留给实施者的决定

实施者可自行决定句子拆分、双语措辞和 census 的表格格式，但不得改变上述范围、source-backed boundary、deferred 清单或历史证据处理策略。
