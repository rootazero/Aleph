# Aleph Phase 2B — B6 实施 Prompt
# Copyable Prompt — Boot Activation and Full Lifecycle Failure Matrix

你是 Aleph 模块化第二阶段 B6 的主 Agent / Lead Orchestrator。工作目录：`.`。

本阶段只做 **Tool/Plugin vertical slice** 的两件事：把 eligible tool-only WASM 的 activation 接到真实 boot/`ensure_loaded` 路径上；把完整 lifecycle failure matrix 与真实边界验收做完。持续推进到验收完成或遇到真实阻塞。**不要自动 pull / push / reset / rebase；提交与 merge 由用户另行要求。**

## 0. North Star — 总体架构目标 / Overall Architecture Goal

> 本节是 B6 的**上位契约 / parent contract**：实现细节与本节冲突时以本节为准；本节**不**扩大本阶段实现范围。完整版见 `docs/research/aleph_modularization_phase2b_b6_plan.md` §0。

**中文** — Aleph 的长期目标不是"一个集成了很多功能的进程"，而是 **轻量 Capability Core + 一组按需启动、可停止、可替换的能力提供者**。Core 常驻，独占 capability descriptor / discovery / control / authority；provider 只是被执行体，不持权威。**Everything externally composable is a Capability**。四条约束：

1. **disabled 的 capability 不得进入 activation、WASM mount、callable publication、provider runtime effect 中的任何一步**，也不得因 catalog / describe / resolve / observer 等**读侧**被隐式启动；读侧只能观察已发布权威状态。
2. **只有 enabled 且 eligible 的 tool-only WASM 才在真实 boot 或显式 enable 路径被激活**；`disable` / `revoke` / `drain` / `dispose` 后不保留 runtime effect（无 VM、无 mount、无 callable entry、无 provider `Ready`、无 retained scope）。**未来 Gateway 的社交媒体通道 / 终端 / 记忆管理等 provider 服从同一 lifecycle contract**，但它们是 B7+ 接入对象。
3. **B6 只证明 Tool/Plugin vertical slice**，不得声称 channel / terminal / memory / MCP / Gateway 等其他 Capability kind 已接入。
4. **技术边界写准**：对 WASM / sidecar 可**可验证地**断言"未 mount / 未 load / 未 start"；对**静态链接进同一进程的 Rust 模块**，**不能**声称代码页从未存在或可热卸载（已随主二进制映射），`disable` 只能撤销 authority 与 effect。该强语义需进程 / 动态加载边界（sidecar / dylib / 子进程），**deferred to B7+**。

**English** — The long-term goal is a **lightweight Capability Core plus on-demand, stoppable, replaceable capability providers**; the Core is resident and solely owns descriptor / discovery / control / authority, while providers are execution bodies holding no authority. **Everything externally composable is a Capability.** Binding rules: (1) a **disabled capability must not enter activation, WASM mount, callable publication, or any provider runtime effect**, nor be started implicitly via read side (catalog / describe / resolve / observer) — read side observes published authority only; (2) **only enabled and eligible tool-only WASM is activated**, via real boot or the explicit enable path; after `disable` / `revoke` / `drain` / `dispose` no runtime effect remains (no VM, no mount, no callable entry, no provider `Ready`, no retained scope) — **future Gateway providers (social channels, terminal, memory management) obey the same contract**, as B7+ targets; (3) **B6 proves the Tool/Plugin slice only** and must not claim any other Capability kind is wired; (4) state the boundary exactly — for WASM / sidecar we can verifiably assert "not mounted / not loaded / not started", while for a **statically linked in-process Rust module** we cannot claim its code pages never existed or can be hot-unloaded; `disable` only withdraws authority and effects, and the strong semantic requires a process or dynamic-loading boundary (sidecar / dylib / subprocess), **deferred to B7+**.

**本阶段成功标准的 North Star 一条（与 plan §1.2 S6 同一条）**：真实 startup / enable / disable 验收中，**disabled provider** 无 VM / mount、无 canonical publication、无 callable entry、provider 非 `Ready`、无 retained runtime resource；**enabled provider** 才有这些效果。断言的是可验证的"未 load / 未 mount / 未 start"，而非"静态链接代码页从未映射"。

---

## 1. 起点事实（先核对，不要假设）

```bash
pwd
git rev-parse --show-toplevel
git rev-parse --short HEAD        # 期望 7b40430f8
git status --short                # 期望 B5 的 8 个未提交修改仍在
git diff --stat
```

- 起点 commit：`7b40430f8`。
- **B5 仍是未提交的工作树变更**（`src/extension/{lifecycle,manager_global,mod,plugin_ops,runtime_owner}.rs` + `docs/reference/{ARCHITECTURE,FEATURE_LOCATOR,TOOL_SYSTEM}.md`）。这是本阶段唯一的基线输入：不得 reset / checkout / stash 丢弃 / `git add -A` / 覆盖他人修改。
- 若不在这份基线上，先停下报告，不要"重建"B5。

## 2. 先读（按顺序，读实际内容）

1. `AGENTS.md`
2. `docs/research/aleph_modularization_phase2b_b6_plan.md` ← 本阶段契约，逐节遵守
3. `docs/research/aleph_modularization_phase2b_plan.md`（B0 六条决策 R1–R6 仍是冻结契约）
4. `docs/research/aleph_modularization_phase2b_prompt.md`
5. `docs/reference/FEATURE_LOCATOR.md` §3.5e 与 Phase 2B 相关段落
6. `docs/reference/ARCHITECTURE.md`、`docs/reference/TOOL_SYSTEM.md` 的 B4/B5 段落
7. `docs/reference/REDLINES.md`、`HARNESS_PHILOSOPHY.md`、`DEVELOPMENT.md`、`STYLE.md`、`SECURITY.md`、`SUBSYSTEM_ROUTING.md`（按需）

## 3. 已冻结的 B5 事实（不得重新解释）

- discovery 对 eligible tool-only WASM 只写 `PluginStatus::Cold`（`src/extension/runtime_owner.rs::declare_cold`，经 `src/extension/lifecycle.rs::discover_and_mount` 的 `eligible` 分支）：不启 VM、不发布 tool surface、registry revision 不变。
- `mount_inner`（`src/extension/lifecycle.rs:733`）遇到已 Cold 的 owner 只 `admit` 并返回 `PluginStatus::Cold`，**不** activation。
- 唯一生产 activation 入口是 `ExtensionManager::set_plugin_enabled`（`src/extension/plugin_ops.rs:873`）：持久化 → `reconcile_plugin_control_locked` → guard 内捕获 exact `RuntimeOwner` incarnation + activation context → **释放 `load_guard`** → `RuntimeOwner::activate_owner`（`src/extension/runtime_owner.rs:205`）→ 重取 guard → 复检 `Arc::ptr_eq` + provider `Ready` + row `Loaded` + `identity`/`receipt` 齐备 + scope 持有 ⇒ `Mounted`，否则 `Failed`（`PluginControlPhase::Reconcile`）。
- retained activation task 只持 `Weak<ExtensionManager>`；未 attach activation context ⇒ fail-closed。`activate_owner` 单飞：`Starting` 不重复 spawn。
- disable ⇒ revoke publication → drain/join 在途 guest 调用 → unload/dispose；陈旧 owner/provider generation/mount identity/lease 一律 fail-closed，残留留在 owner 的 `OwnerFailure::*`。
- `ToolHandlerRegistry` 是唯一 callable authority；`ZahirFacade` 是 subscribe-before-snapshot + live broadcast + lag 时 `Invalidated` resync；`SessionEventStore` 仍是唯一 session durability。
- **已明确 deferred**：boot auto-activation、full failure-matrix acceptance（= 本阶段）、MCP lazy、`EffectClaim` producer、Safe Replay、universal scheduler、其他 Capability kinds、external-effect exactly-once、`src/harness/`。

## 4. 结构性约束（读代码时必须自己复现这两条）

- **C1 死锁**：`mount_owner` 自己取 `load_guard`。任何在持有 `load_guard` 的调用栈里同步等待 activation 的写法会**挂死**而非报错。因此 boot activation 不能放进 `discover_and_mount` / `load_all_locked` / `mount_inner`，只能在 guard 释放后按 `set_plugin_enabled` 同形状执行（capture → drop guard → activate → 重取 → 复检）。
- **C2 fast path**：`ensure_loaded`（`src/extension/mod.rs:677`）在 `cache_state.loaded` 为真时直接返回；`load_all_locked` 在 discovery 末尾置 `loaded = true`。boot activation 与此置位时机的先后必须显式定义。
- **C3 并发入口**：启动期至少 4 处并发 `ensure_loaded`（`agent_init/mod.rs:677`、`tool_catalog_init.rs:162`、`run_loop/mod.rs:636`、`gateway/handlers/plugins/handlers/manage.rs:31`），另有 `builtin_tools/plugin_manage.rs:213`、`hub/reconcile.rs:78`。
- **C4 测试脚手架**：`RuntimeOwner.publication_barrier` / `.barrier` / `.stop_join_entered` 是 `#[cfg(test)]`（`src/extension/runtime_owner.rs:64-80`）；不得为测试打开生产字段。

## 5. 本阶段目标

1. **B6.0（只读，先做）**：source-backed 审计 `ensure_loaded`、`load_all_locked`、`discover_and_mount`、`mount_inner`、`set_plugin_enabled`、`activate_owner`、`mount_owner`、gateway/plugin-control/startup 入口，产出冻结决策：
   - **D-A** boot activation 的唯一生产入口（推荐：`ensure_loaded` 两段式；选项与代价见 plan §3.3）。
   - **D-B** 多插件 boot activation 失败策略：**per-plugin fail-closed** 还是 **fail-fast**。把选项、证据、代价列给用户，**等用户确认**；不要静默选一种继续写代码。
   - **D-C** B6 之后 `Cold` 仍在哪些路径可见。
   - **D-D** 第二次 `load_all` / reload 与 boot activation 的交互。
2. **B6.1** boot auto-activation：enabled 且 eligible 的 Cold owner 在真实 boot 路径完成 activation；读侧（catalog/describe/resolve/`plugins.list`/observer/doctor）**绝不**触发；明确 `load_guard`、owner incarnation、provider generation、`WasmMountIdentity`、canonical `Ready` commit、失败清理；复检逻辑与 `set_plugin_enabled` 共用**一份** helper。
3. **B6.2** 失败矩阵：按 plan §5 的 18 个阶段逐个落地 outcome / publication / resource / observer 四语义；`Mounted`/`Loaded`/`Ready` 三者任一出现时另两者必须已成立。
4. **B6.3** 并发/替换/observer 验收：真实 WASM fixture + `Barrier`/`Notify`/`oneshot`，覆盖 X1–X9；禁止 sleep 排序、mock-only、只统计调用次数。
5. **B6.4** 真实边界 QA 与验证矩阵（见 §7）。
6. **B6.5** 文档与验收报告（见 §8）。

## 6. 执行边界

- 主 Agent 负责方案、决策记录、writer 边界、diff 审查、验收；实现与大规模测试优先委派 subagent。
- 并行只读审计（Explore/researcher）可任意并行；**同一文件同一时刻只允许一个 writer**。多个 subagent 不得同时改同一文件，也不得跨任务扩张修改范围。
- 委派任务必须自带：目标、允许改动文件、必要上下文、约束、交付物、验收标准、前置依赖。
- Subagent 声称完成 ≠ 完成。主 Agent 必须读实际 diff 并重跑关键验证。

## 7. 内存门与最低验证集合

每一条 cargo 命令**之前**都必须单独执行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有两者 exit 0 且 `available_bytes >= 4294967296` 才允许 cargo；cargo **串行**，不并发共享 target。

按实际 diff 选择并逐条记录 exit code、真实测试数、warning：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib extension::lifecycle::
cargo test -p alephcore --lib extension::plugin_ops::
cargo test -p alephcore --lib extension::runtime_owner::
cargo test -p alephcore --lib tools::registry::
cargo test -p alephcore --lib capability::zahir_facade::
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy --workspace --all-targets
```

- filter 命中 0 tests 不算通过；旧日志不算通过；未跑写 `UNRUN`；内存门失败/fixture 缺失写 `BLOCKED` 并说明原因。
- 已知基线 warnings（不得记为干净通过）：`src/extension/plugin_ops.rs:877` large `Err` 变体 + 两处既有 registry 测试 lint。新增 warning 必须解释。

## 8. 禁止事项（违反即停止）

- 修改 `src/harness/`。
- 新建第二 callable registry、第二 enabled 判定/durable store、第二 actual-state authority、独立 disposer map。
- MCP lazy、sidecar、dylib、scheduler、Safe Replay、`EffectClaim` producer、跨 Capability kinds、external-effect exactly-once。
- 读侧触发 activation；用 metadata/catalog/role/`TurnContext`/name fallback 合成 authority；引入 activation proxy 让 canonical 首次调用隐式启动 Cold。
- 把 `#[cfg(test)]` barrier 打开成生产字段；用 sleep 排序或 mock-only 通过验收；伪造 registry revision；把 provider broadcast 当 durable recovery。
- 放宽 `eligible`、跳过 admission、跳过 drain、吞掉 partial failure / dispose residual。
- 自动 commit / push / reset / rebase / stash 丢弃 / 覆盖共享工作树内容。

## 9. 停止并报告的条件

- D-B（fail-fast vs per-plugin）未获用户确认。
- 需要改变 B5 已冻结的 Cold/Ready/stop 权限语义。
- 需要新增 activation proxy 才能达成 boot 目标。
- 只能靠绕过既有 permission/visibility/trust/confirmation 或伪造 revision 让测试通过。
- 内存门失败、真实 WASM fixture 不可用、共享 writer 冲突、连续三次无实质进展。
- 发现 C1/C2 之外的第二个结构性阻塞（例如 `ensure_loaded` 语义扩展破坏既有调用点契约）。

## 10. 最终报告字段（缺一不可）

- 起点核对结果（HEAD、`git status --short`、B5 未提交状态）。
- B6.0 的调用图（带 `path:line`）与 D-A/D-B/D-C/D-D 的选项、证据、用户确认结论、代价。
- 修改文件清单与每条的真实生产调用链。
- 失败矩阵四语义（outcome/publication/resource/observer）的落地位置：阶段 → 测试名 → 断言内容。
- 每条 cargo 命令的 memory gate 结果、exit code、passed/failed/ignored、warnings。
- B5 保持的行为 / B6 新完成的行为 / 仍 deferred 的行为。
- **North Star 一致性 / North Star conformance**：逐条回答 §0 的四条命题（disabled 是否未进入 activation/mount/publication/effect、读侧是否未启动、enabled 才有效果；是否仅限于 Tool/Plugin；对静态链接模块是否避免虚假的"代码不在内存"断言）；若任一条不成立，如实报告，不得以"后续阶段"掩盖。
- 残余风险、BLOCKED/UNRUN 清单。
- **明确列出没有做什么**（未跑的验证、未覆盖的边界、未修改的子系统）。
- 未提交、未推送状态（本阶段不变更 git 历史）。
