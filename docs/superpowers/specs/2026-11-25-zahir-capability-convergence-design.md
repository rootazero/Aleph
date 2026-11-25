# Zahir Capability 收敛与依赖感知生命周期 / Zahir Capability Convergence and Dependency-Aware Lifecycle

- 日期 / Date: 2026-11-25
- 分支 / Branch: `zahir-capability-convergence`（worktree `.worktrees/zahir-convergence`，基于 main `a4dac8db4`）
- 状态 / Status: 已批准（2026-11-25）/ Approved by the user（§5 修正已确认；§6.3 提交原子性与排空超时为设计者决定）
- 上游 / Upstream: `docs/research/Everything-externally-composable-is-a-Capability.md`（§10 工具集会话固定、§10.3 热替换、§13 成功标准）；参考实现 cordis、cordis-rs、pi/packages/chord

## 1. 意图 / Intent

把「Everything externally composable is a Capability」在 Zahir 中落到**单一权威**：同一事实只有一处表述，生命周期由一套状态机承载，依赖缺失时调用方得到可读、可恢复的答案，替换插件不再以「先拆后装」冒丢失旧版本的风险。

Phase 6–8 留下了并存权威与未挂载模块（见 §3）。在它们之上继续加功能会把熵放大；所以本期先收敛，再在剩下的那一套机制上扩展。

## 2. 目标 / 非目标 / 反目标

**目标 / Goals**

- G1 删除与生产权威重复或未挂载的 capability 模块，净删除行数显著为正（§13：删掉的旧结构 ≥ 新增组件）。
- G2 依赖不可用时，调用返回结构化「暂时不可用」错误（含依赖 id 与其状态），而不是与「不存在」同形的 `ResolveError::Unknown`。工具列表不变（§10）。
- G3 插件按 id 分阶段替换：新版本先准备并证明可用，再原子切换；准备失败时旧版本原样继续服务；已准入的调用在旧实现上跑完。
- G4 FEATURE_LOCATOR 与 `docs/reference` 反映收敛后的代码事实。

**非目标 / Non-goals**

- 不迁移通道、pty、插件各自的 provider 到 `capability::ProviderRegistry`。
- 不统一四个同名 `ProviderRegistry`（`src/capability`、`src/providers`、`src/thinker`、generation）——不同领域，改名只增改动量。
- 不做 Safe Replay、external-effect exactly-once、通用 durable scheduler、EffectClaim producer。
- 不改 `src/harness/`（R10 棘轮）。

**反目标 / Anti-goals**

- 不新建 lifecycle supervisor / Fiber 式统一调度器（方案二已否决：R10 薄 harness）。
- 不在会话中途增删工具（会破坏 prompt cache，违反 §10）。
- 不为单一消费者引入通用 trait 或 registry（P6、R3）。
- 不保留「报成功的 no-op」：没有消费者的机制不进入本期（判据 11）。

## 3. 源码事实 / Source facts（HEAD `a4dac8db4`，rg 核实）

| 事实 | 位置 | 结论 |
|---|---|---|
| `LegacyCapabilityHandlerRegistry` 与生产 `special::CapabilityHandlerRegistry` 并存，`backend.rs:33-38` 以同名重导出 | `src/capability/backend.rs:117,251`；`src/capability/special.rs:439,1065` | Legacy 仅 `ambient.rs` 与测试使用 → 删 |
| `ambient.rs` 自述 DEFERRED, NOT MOUNTED | `src/capability/ambient.rs:3-11` | 移出源码树，git 历史可找回 |
| `audit.rs` 自述 NOT MOUNTED；生产审计是 `SecurityAuditLog` | `src/capability/audit.rs:3-8`；`src/capability/projection_host.rs:858` 仅测试调用 | 删 |
| `grant.rs` 仅集成测试使用；生产授权是 sandbox `GrantStore` | `src/capability/grant.rs:3-8`；`src/sandbox/exec_approval/grants.rs:186` | 删；`policy.rs:365` 的跨会话授权推迟只留文档记录 |
| `lifecycle.rs` + `activation.rs` 的状态机，生产只经 `policy_runtime.rs:146` 使用，`ttl=None`、空 `ConstraintSet` | `src/capability/policy_runtime.rs:133-195` | 实际只用到「代数比对 + 已撤销」两项 → 换成 `PolicyAttachment` 内的最小检查后删除两文件 |
| scoped 的 `capability_activations` 是兼容入口，生产刻意不安装 | `src/tools/scoped/dispatch.rs:1531`；`builder.rs:91` 仅测试调用 | 删 |
| `ActivationEventHook` 只由 `#[cfg(test)] new_observed` 构造 | `src/capability/events.rs:920-1000`；`activation.rs:110` | 随 activation 删 |
| `ownership.rs:5` 称 ownership 树「推迟到 Task 2」 | `src/capability/ownership.rs:5` | 过时注释，修正 |
| memory 未运行时 `capture` 返回 `ResolveError::Unknown` | `src/memory/runtime/mod.rs:363-366` | 与「不存在」同形，模型无法区分「暂时不可用」 |
| `resolve_resource` 只认 memory | `src/capability/zahir_facade.rs:1671-1687` | 依赖目标的「是否可用」判定应来自 `ProviderRegistry` 中的 provider 状态 |
| 当前全部 `Requires` 边都是「memory 工具 → `aleph/resources/memory.runtime`」 | `src/builtin_tools/{memory_search,memory_browse,…}.rs` 共 8 处 `required_service()` | 没有 provider→provider 边 |
| `ProviderHandle::subscribe_changes` 无外部订阅者 | `src/capability/provider.rs:408` | — |
| `reload_plugin` = 先 `unmount_inner` 再 `mount_inner`；新版本挂载失败时插件消失 | `src/extension/lifecycle.rs:288-305` | G3 的改造点 |
| 已有原子替换原语：`publish_plugin_batch_with_ready_replacement(batch, replacing)`、`ToolHandlerRegistry::replace`、hook/panel publication replace | `src/tools/registry.rs:1409,1685`；`src/extension/runtime_owner.rs:417-420`；`src/extension/panel_publication/mod.rs:71` | 复用，不新建 |
| `ProviderHandle::with_admitted_completion` 标 `#[allow(dead_code)]`，为「已准入调用在 Stopping 中完成」预留 | `src/capability/provider.rs:367` | G3 给它第一个生产消费者 |
| `runtime_owners` 与 WASM loader 都以 plugin id 为键 | `src/extension/mod.rs:343`；`src/extension/loader.rs:262`；`src/extension/runtime/wasm/mod.rs:499` | 新旧两版共存需要按代数区分的键（见 §6.3） |

## 4. 方案 / Approaches（已与用户确认）

- **方案一（选定）**：先收敛，再在同一套机制上扩展；同一分支分段提交，分阶段替换放进本期。
- 方案二：照 cordis-rs Fiber 写统一 supervisor 接管全部 provider —— 否决：改动约 20 个子系统，违反 R10。
- 方案三：本期只收敛 —— 否决：功能增强落空。

## 5. 对已批准方案的两处修正（用户已于 2026-11-25 确认）/ Corrections confirmed by the user

写设计时按源码复核，发现上一轮方案里有两点没有生产消费者，按判据 11（报成功的 no-op）不应原样实现：

1. **`ProviderState::Parked` + 依赖恢复时重新拉起下游。** 当前所有 `Requires` 边的下游都是工具，不是 provider；工具在会话内固定列出（§10），不需要也不应「停放」。实现一个监听 provider→provider 边的循环，在今天会零次触发。
   **决定**：本期把「等待依赖」落在**调用路径**上——G2 的结构化 `DependencyUnavailable` 错误；`Parked` 状态与反向依赖重查写入本文 §9「推迟项」，触发条件是第一条 provider→provider 边出现。
2. **`resolve_resource` 改为通用 provider 查找。** 可用性判定可以通用（读 `ProviderRegistry` 中目标 provider 的状态），但「捕获资源句柄」只有 memory 一个实现；为它抽 trait 是单消费者抽象。
   **决定**：判定通用化，捕获保持 memory 专有分支。
3. **段 S 的双实例切换改为「先预检新版本，再拆旧版本」**（实施计划 §S0，用户于 2026-11-25 确认）。hooks/panel 由 `after_transition` 整体重建，loader / WASM runtime / `runtime_owners` / `scopes` 均以插件 id 为键，MCP 服务器与 services 不能新旧双开，§6.3 的双实例提交需要重设计这些键与双开语义，超出本期。
   **决定**：把可失败且无副作用的步骤移到拆除之前——解析、身份校验、WASM 模块编译（`CompiledPlugin::new`，只编译不实例化，不运行任何 guest 代码）。预检失败且有版本在运行时，什么都不拆、不改行，返回 `MountError::ReloadRefused`（文案「the previous version is still running」）。预检通过后走现有 unmount → mount。
   **实施时的一处收窄 / Narrowed during implementation**：准入被拒（信任策略或 `plugins.toml`）不算「新版本有缺陷」，而是运营者的决定；若因此保留旧版运行，等于让已被撤销信任的插件继续跑（闸的放行方向）。所以准入拒绝仍沿用今天的语义：拆除并在行上记录拒绝。/ An admission refusal is the operator's decision, not a defect of the new version; it still tears down and records the refusal, so a reload never keeps a revoked plugin running.

## 6. 设计 / Design

### 6.1 段 C：收敛 / Convergence（第一段提交）

删除：

- `src/capability/backend.rs` 中 `LegacyCapabilityHandlerRegistry` 及 `Legacy*` 类型、`:33-38` 同名重导出、对应测试；`ToolBackendAdapter` 保留。
- `src/capability/ambient.rs`、`src/capability/audit.rs`、`src/capability/grant.rs`、`src/capability/grant_integration_tests.rs`；`projection_host.rs:858 audit_collector`；`events_tests.rs` / `events_scoped_tests.rs` 中依赖 audit 的测试。
- `src/capability/lifecycle.rs`、`src/capability/activation.rs`、`events.rs` 中 `ActivationEventHook`。
- `src/tools/scoped/{mod.rs:122, builder.rs:91-94, dispatch.rs:1531-1560}` 的兼容激活入口及其测试（`memory_runtime_tests.rs:203,231,254`、`tests.rs:5094,5251`）。
- `src/capability/mod.rs` 中对应 `pub mod`。

替换：

- `PolicyAttachment`（`policy_runtime.rs:133`）去掉 `Mutex<CapabilityActivationController>`，改为 `revoked: AtomicBool`。`admit` 中只检查 `!revoked` 且调用上下文的 generation 等于 `binding.generation`；拒绝理由保留现有 `denied("runtime_lifecycle")` 文案，使外部可观察行为不变。
- 修正 `src/capability/ownership.rs:5` 过时注释。

验收：`cargo check` / `cargo test -p alephcore` 中 capability、tools::scoped、gateway 相关测试全绿；`census.rs` 的静态守卫全绿；净删除行数报告。

### 6.2 段 D：依赖不可用的结构化答案 / Dependency availability（第二段提交）

- `ResolveError` 增加 `DependencyUnavailable { target: CapabilityId, state: ProviderState }`（闭集扩一项，所有 `match` 逐个处理；`zahir_facade.rs:87` 的 `NotVisible → Unknown` 折叠不得吞掉新变体）。
- `resolve_resource`：先在 `ProviderRegistry` 中查目标；不在 → `Unknown`；provider 状态不是 `Ready` → `DependencyUnavailable`；`Ready` → 走现有 memory 捕获。`MemoryRuntime::capture` 的 `MemoryState::Running` 检查保留为二次栅栏（竞态时仍 fail-closed）。
- 工具调用路径把 `DependencyUnavailable` 映射成模型可见的错误文本：「`memory.runtime` 当前不可用（状态 Stopped），稍后重试或请用户启用」。工具列表与 schema 不变。
- 事件标签（`zahir_facade.rs:1471` 一带）增加 `dependency_unavailable`。

### 6.3 段 S：插件分阶段替换 / Staged plugin reload（第三段提交，可单独回退）

`reload_plugin(id)` 改为三步，全程持有 `load_guard`：

1. **准备 / Prepare**：重新发现并解析该 id 的 manifest，加载新代码（WASM 模块等）到**未发布**的暂存实例，建立新 `RuntimeOwner`（新 provider 代数，`Starting`）并准备 batch。旧 owner 原样服务。
2. **失败即放弃 / Abort**：任一步失败 → 反序处置暂存实例的 `EffectScope`，旧版本不受影响；返回错误，文案明确「旧版本仍在运行」。
3. **提交 / Commit**（全有或全无 / all-or-nothing，设计者决定）：准备阶段已生成全部发布面的新载荷且不发布。提交时先替换 hooks、panel publication（各自现有 replace，记下被替换的旧值），最后用 `publish_plugin_batch_with_ready_replacement(batch, Some(old_receipt))` 原子切换工具发布，它是切换的线性化点；随后 `runtime_owners[id]` 换成新 owner。线性化点之前任一步失败 → 按逆序恢复已替换的发布面，处置暂存实例，旧 owner 继续服务。不允许对外出现新旧混合的版本。
4. **排空旧版 / Drain old**（设计者决定）：旧 provider `Ready → Stopping`，停止准入；已准入调用经 `with_admitted_completion` 完成。排空有上限超时；超时后**不新增语义**，沿用今天 `unmount_inner` 对进行中调用的处理（调用不被主动取消；provider 进入 `Stopped` 后其结果按现有规则丢弃，见 `src/tools/registry.rs` 测试 `replacement_discards_output_from_an_already_admitted_handler`），并在重载报告里记为残留。最后反序 dispose 旧 `EffectScope`。

键冲突：`runtime_owners`、loader、WASM runtime 目前以 id 为键。共存窗口需要「id + provider 代数」区分新旧实例；由实现者定位最小改动点。若某类插件（如纯 Markdown 静态插件）没有运行时代码，准备阶段即是解析，不需要双实例。

`reload`（全部插件）不在本段范围，保持现状。

> **已按 §5 第 3 条修正 / Superseded by §5 item 3.** 以上 1–4 步的双实例提交本期未实现；实际实现见 `src/extension/lifecycle.rs` 的 `reload_plugin` / `preflight_reload` / `prepare_mount` / `commit_mount`，以及 `src/extension/runtime/wasm/mod.rs` 的 `WasmRuntime::preflight_module`。预检覆盖不到的失败（模块能编译但链接/实例化失败、MCP 服务器或 service 启动失败）仍发生在拆除之后，行为与今天一致。

### 6.4 段 F：文档 / Docs（最后一段提交）

- FEATURE_LOCATOR「当前状态」更新到本期结束；登记 Phase 6–8 模块的现状（含被删模块与 git 找回方式）；§3.5 闭环记录追加本期条目。
- `docs/reference/{ARCHITECTURE, TOOL_SYSTEM, PLUGIN_SYSTEM, GLOSSARY}.md` 中涉及被删模块、激活、插件重载的段落按代码校对。
- 证据写入 `docs/superpowers/runs/2026-11-2x-zahir-convergence-receipt.md`。

## 7. 留给实现者 / Left to the builder

- 段 S 中按代数区分键的具体落点（loader / WASM runtime / `runtime_owners`）与排空超时数值。
- 段 D 中 `ResolveError` → 工具错误的映射位置（scoped dispatch 或 facade 层），只要模型看到的文本与 §6.2 一致。
- 被删测试中若有仍在保护生产行为的断言，迁到对应生产模块的测试里，而不是随文件一起删。

## 8. 验证 / Verification

- 每段提交前：`cargo check --workspace`、`cargo clippy`（受影响 crate）、相关模块 `cargo test`。
- 段 D：单测覆盖 memory `Stopped` / `Starting` / `Failed` 下工具调用得到 `DependencyUnavailable`；`Ready` 下行为不变。
- 段 S：单测覆盖 (a) 新版本解析失败时旧版本工具仍可调用；(b) 切换后新调用走新实现；(c) 切换前准入的调用在旧实现上完成；(d) 排空超时路径报告残留。
- 合并前独立审阅（不同模型），列出未做事项。

## 9. 推迟项 / Deferred（附触发条件）

| 项 | 触发条件 |
|---|---|
| `ProviderState::Parked` + 依赖恢复时重拉下游 | 第一条 provider→provider `Requires` 边进入生产 |
| `resource` 捕获的通用 trait | 第二种被 `Requires` 的资源出现 |
| 全量 `reload` 分阶段化 | 单插件分阶段替换稳定后 |
| 单插件双实例切换 + 旧版排空（`with_admitted_completion` 首个消费者、按 id+代数分键） | 出现预检拦不住的高频重载失败 |
| 跨会话 grant（`policy.rs:365`） | sandbox `GrantStore` 有跨会话需求 |
| ambient capability 重新挂载 | 安全面评审通过 |
