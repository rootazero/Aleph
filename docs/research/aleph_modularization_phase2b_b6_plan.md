# Aleph 模块化 Phase 2B — B6 计划
# Phase 2B B6 Plan — Boot Activation and the Full Lifecycle Failure Matrix

**起点 / Starting point:** `7b40430f8`（B5 变更仍为共享工作树未提交修改：`src/extension/{lifecycle,manager_global,mod,plugin_ops,runtime_owner}.rs` + `docs/reference/{ARCHITECTURE,FEATURE_LOCATOR,TOOL_SYSTEM}.md`）。
**范围 / Scope:** 只继续 Tool/Plugin vertical slice。B6 **不**扩展 MCP lazy、`EffectClaim` producer、Safe Replay、universal scheduler、其他 Capability kinds、Gateway/Terminal/Memory backend、external-effect exactly-once、`src/harness/`。

---

## 0. North Star — 总体架构目标 / Overall Architecture Goal

> 本节是 B6 的**上位契约 / parent contract**。B6 的实现细节若与本节冲突，以本节为准；本节**不**扩大 B6 的实现范围。

### 0.1 中文 / Chinese

Aleph 的长期目标不是"一个集成了很多功能的进程"，而是 **轻量 Capability Core + 一组按需启动、可停止、可替换的能力提供者**。Core 常驻，独占 capability descriptor、discovery、control 与 authority；能力提供者只是被执行体，不持有权威。总原则：**Everything externally composable is a Capability**——任何需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的东西，都必须通过 Capability 契约表达，而不是以"它恰好被链接进这个进程"的方式隐式存在。

由此推出四条对本阶段有约束力的命题：

1. **Core 常驻并持有 authority。** capability descriptor / discovery / control / authority 只在 Core 内。**disabled 的 capability 不得进入 activation、WASM mount、callable publication、provider runtime effect 中的任何一步；也不得因为 catalog / describe / resolve / observer 等读侧被隐式启动。** 读侧只观察已发布的权威状态，永远不能成为启动触发器。
2. **只有 enabled 且 eligible 的 tool-only WASM 才被激活**，入口限于真实 boot 或显式 enable 路径。`disable` / `revoke` / `drain` / `dispose` 之后不保留 runtime effect——没有 VM、没有 mount、没有 callable entry、没有 provider `Ready`、没有 retained scope。**未来 Gateway 下的社交媒体通道、终端、记忆管理等 provider 服从同一 lifecycle contract**；它们是 B7+ 的接入对象，不是第二个生命周期系统。
3. **B6 只证明 Tool/Plugin vertical slice。** 不得声称 channel / terminal / memory / MCP / Gateway 等其他 Capability kind 已接入该契约——它们在本阶段既未实现也未验收。
4. **"代码不被拉入主进程内存"的技术边界必须写准。** 对 WASM / sidecar 这类具有独立执行载体的 provider，我们可以**可验证地**断言"未 mount / 未 load / 未 start"（VM 未创建、实例未实例化、进程未 spawn）。对**静态链接进同一进程的 Rust 模块**，我们**不能**声称其代码页从未存在过，也不能声称可以热卸载——它已随主二进制映射进地址空间；`disable` 在那里只能撤销 authority 与 effect（不发布 entry、不 retain scope、不提供 runtime），不能回收已映射的代码页。若某能力确实需要"未启用时连代码都不在内存"的强语义，就必须引入进程边界或动态加载边界（sidecar / dylib / 子进程），该边界**明确 deferred 到 B7 及以后**，不属于 B6。

### 0.2 English

The long-term goal is not "a process that integrates many features" but a **lightweight Capability Core plus a set of on-demand, stoppable, replaceable capability providers**. The Core is resident and solely owns capability descriptor, discovery, control and authority; providers are execution bodies and hold no authority. Governing principle: **Everything externally composable is a Capability** — anything that an Agent, the Runtime, or another system must dynamically discover, invoke, replace, subscribe to, or manage has to be expressed through the Capability contract, never implicitly by "it happens to be linked into this process".

Four propositions binding on this phase:

1. **The Core is resident and holds authority.** descriptor / discovery / control / authority live only in the Core. **A disabled capability must not enter activation, WASM mount, callable publication, or any provider runtime effect; nor may it be started implicitly through read-side paths (catalog / describe / resolve / observer).** Read side observes published authority only and must never act as an activation trigger.
2. **Only enabled and eligible tool-only WASM is activated**, and only via real boot or the explicit enable path. After `disable` / `revoke` / `drain` / `dispose`, no runtime effect is retained — no VM, no mount, no callable entry, no provider `Ready`, no retained scope. **Future Gateway providers (social channels, terminal, memory management) obey the very same lifecycle contract**; they are B7+ onboarding targets, not a second lifecycle system.
3. **B6 proves the Tool/Plugin vertical slice only.** It must not claim that channel / terminal / memory / MCP / Gateway or any other Capability kind is wired to this contract — none of them are implemented or accepted in this phase.
4. **The "code is not pulled into the main process's memory" boundary must be stated exactly.** For providers with an independent execution carrier (WASM / sidecar) we can **verifiably** assert "not mounted / not loaded / not started" (no VM created, no instance instantiated, no process spawned). For a **statically linked in-process Rust module** we **cannot** claim its code pages never existed, nor that it can be hot-unloaded — it is already mapped into the address space with the main binary. `disable` there can only withdraw authority and effects (no published entry, no retained scope, no live runtime); it cannot reclaim mapped code pages. If a capability genuinely needs the strong semantic "not even code in memory while disabled", that requires a process or dynamic-loading boundary (sidecar / dylib / subprocess), which is **explicitly deferred to B7+** and out of B6's scope.

---

## 1. 目标、成功标准、假设与 trade-off / Goal, Success Criteria, Assumptions, Trade-offs

### 1.1 目标 / Goal

在 B5 已封边的基础上完成两件事，并把它们变成**可被真实启动路径和真实失败路径证明**的契约：

1. **boot activation**：operator 已 enable 的 eligible tool-only WASM，在真实 boot / `ensure_loaded` 路径上完成 activation，而不是停在 `Cold` 等一次人工 enable。
2. **full lifecycle failure matrix / real boundary acceptance**：discovery → admission → Starting → mount → binding/export → revalidation → publication → dispatch → disable → drain/join → unload/dispose → replacement/reload → stale completion 的每一阶段，都有冻结的 outcome / publication / resource / observer 语义，并且有真实 WASM fixture 的可执行证据。

### 1.2 成功标准 / Success criteria

- S1：全新进程、插件在 `plugins.toml` 中 `enabled = true`，**只**经由既有启动接线（`ensure_loaded` 调用点族）即可观察到：provider `Ready`、registry row `Loaded`、`RuntimeOwner.identity` 与 `.receipt` 齐备、canonical `ToolHandlerRegistry` 中存在该插件 entry，且一次真实 canonical dispatch 返回 guest 的确定 payload。禁止测试直接调用 `set_plugin_enabled(true)` 充当 boot 证据。
- S2：boot 期间**任何**读侧入口（catalog / `plugins.list` / describe / resolve / Zahir `describe`+`subscribe` / doctor / activation gate）都不触发 activation；它们只观察已发布的权威状态。
- S3：activation 失败在 boot 路径上产生**结构化 Failed/residual**（owner `failures` + registry row `Error` + provider `Failed`），永不出现 "row `Loaded` 但无 receipt" 或 "receipt 在而 provider 非 Ready" 的 false Ready。
- S4：失败矩阵中每一阶段都有命名测试，断言的是**效果到达**（VM 未创建 / entry 未发布 / scope 已 dispose / revision 未递增 / observer 收到 `Invalidated`），不是 "函数被调用"。
- S5：同一 worktree 里 `cargo fmt --all --check`、定向 lib 测试、`cargo test -p alephcore --lib`、`cargo check -p alephcore -p aleph-slot-api --all-targets`、`cargo clippy --workspace --all-targets` 全部有记录（含 warnings / ignored / UNRUN）。
- S6（North Star 边界验收 / North Star boundary acceptance）：在**真实 startup / enable / disable 验收**中，disabled provider 不产生 VM 或 mount、不产生 canonical publication、不存在 callable entry、provider 不进入 `Ready`、不保留 runtime resource（scope / activation task / admission 全部已释放）；enabled provider 才具备这些效果。对 WASM 断言的是"未 load / 未 mount / 未 start"这一**可验证事实**，**不**断言"静态链接的 Rust 代码页从未映射进进程"——该强语义需要进程或动态加载边界，见 §0 第 4 条与 §10，属 deferred。

### 1.3 假设 / Assumptions

- A1：B5 的工作树状态是本阶段的唯一基线；任何 "B5 已提交" 的假设都是错的（HEAD 仍是 `7b40430f8`）。
- A2：`RuntimeOwner` 仍是 WASM lifecycle 的唯一 owner；`ProviderHandle` 仍是 provider state/generation 权威；`ToolHandlerRegistry` 仍是唯一 callable authority。B6 不新建其中任何一个的第二份。
- A3：eligible 判定（`src/extension/runtime_owner.rs::eligible`，窄 allowlist：kind=Wasm、capabilities 全为 Tool、无 hooks/services/commands/memory/prompt、无 dynamic tools/hooks/workspace/http/secrets、无 aleph extensions、`wasm_resource_limits.timeout_secs > 0`）在 B6 保持不变。B6 不扩大 eligible 面。
- A4：真实 WASM fixture 可用（`src/extension/lifecycle.rs:1720::write_cold_tool_wasm_fixture` → `crate::extension::runtime::wasm::tests::guest_fixture::{manifest_with_guest, write_barrier_guest, PAYLOAD_A}`），内存门可执行。

### 1.4 Trade-offs（本设计让什么变难 / what this makes harder）

- T1：boot 期做 activation 会让**启动时长**与插件数量线性相关（每个 enabled 插件一次 VM 起停 + mount）。缓解：单次 `ensure_loaded` 后一次 activation pass、per-owner single-flight（`activate_owner` 的 `Starting` 分支），不做重试风暴；但"boot 更慢"是接受的代价，不引入后台重试调度器（那属于 universal scheduler，B6 禁止范围）。
- T2：boot activation 失败必须**不**污染整机启动（见 B6.0 决策 D-B）。代价是：一次失败之后，同一进程内 `cache_state.loaded = true` 的 fast path 会让后续 `ensure_loaded` 不再回头修它——修复只能靠显式 enable 面或 reload。这是接受的行为，但必须写进文档，不能让用户以为"重启前自愈"。
- T3：把 activation 放在 `load_guard` 之外（结构性必须，见 §3.2）意味着 boot activation 与并发 enable/disable 之间存在真实竞态，只能靠 owner incarnation + generation + publication fencing 收敛，不能靠"boot 期间没人动"的假设。代价是 B6.3 的并发测试矩阵无法压缩。
- T4：失败矩阵写全之后，任何新 lifecycle 阶段都必须同时补四个语义（outcome / publication / resource / observer），后续阶段的实现成本上升——这是刻意的：形状 5（列举法只覆盖立法当天的世界）正是 B6 要关掉的漏洞。

---

## 2. B5 已完成基线 / B5 Baseline (source-backed)

### 2.1 权威分层 / Authority layers

| 事实 / Fact | 唯一权威 / Sole authority | 证据 / Evidence |
|---|---|---|
| WASM lifecycle、owner incarnation、effect scope | `RuntimeOwner` | `src/extension/runtime_owner.rs:60-180` |
| provider state + generation | `ProviderHandle` | `src/extension/runtime_owner.rs`、`src/capability/provider.rs` |
| callable tool entry / descriptor / revision | `ToolHandlerRegistry` | `src/tools/registry.rs:1016`（`publish_plugin_batch`）、`:1030`（`publish_plugin_batch_with_ready_commit`）、`:778`（`revoke`）、`:50`（`ReadySnapshot`）、`:678`（`PluginBatch`） |
| plugin row / desired state | `PluginRegistry` + `plugins.toml` | `src/extension/plugin_state.rs`、`src/extension/lifecycle.rs:733` |
| projection / notification | `views().after_transition()` + registry broadcast + Zahir | `src/extension/lifecycle.rs::publish_runtime_transition_views`、`src/capability/zahir_facade.rs` |
| session durability | **只有** `SessionEventStore` | 未变 |

### 2.2 已封边的生产事实 / Already-frozen production facts

1. discovery 对 eligible tool-only WASM 只写 `PluginStatus::Cold`：`src/extension/lifecycle.rs::discover_and_mount` 的 `Ok(()) if runtime_owner::eligible(..)` 分支 → `declare_cold`（`src/extension/runtime_owner.rs:188`），不启 VM、不发布 entry、revision 不变。
2. `mount_inner`（`src/extension/lifecycle.rs:733`）遇到已存在的 Cold owner：`admit` 后**直接返回 `PluginStatus::Cold`**，不 activation。
3. 唯一生产 activation 入口是 `ExtensionManager::set_plugin_enabled`（`src/extension/plugin_ops.rs:873`）：持久化 → `reconcile_plugin_control_locked`（`src/extension/lifecycle.rs:164`）→ 若 outcome 为 `Cold` 则在同一 `load_guard` 内捕获 exact owner + activation context → **释放 guard** → `RuntimeOwner::activate_owner`（`src/extension/runtime_owner.rs:205`）→ 重新取 guard → 复检 `Arc::ptr_eq(current, owner)` && `provider == Ready` && row `Loaded` && `identity.is_some()` && `receipt.is_some()` && `scopes.contains_key(id)` ⇒ `Mounted`，否则 `Failed` / `Phase::Reconcile`。
4. `activate_owner`：`Cold → Starting`（唯一合法迁移），spawn retained task 调 `mount_owner(owner, generation)`；`Starting` 分支不重复 spawn，只等 `completion` watch；其余状态返回 `plugin owner rejects activation: {state:?}`。
5. `mount_owner`：stop fence select →（`#[cfg(test)]` barrier）→ 取 `load_guard` → 复检 `owner_is_current` + `(Starting, generation)` → `admit` → 若 loader 已有该 id 的 runtime 则 `"activation found an unowned runtime; explicit recovery required"` → row `Starting` → `mount_parsed` → 读 `wasm_mount_identity` → 复检 owner/generation → 再 `admit` → 写 `identity` → `prepare_owner_batch` → `publish_plugin_batch_with_ready_commit`；发布失败：`OwnerFailure::Publication`、移除并 dispose scope、provider `Failed`、row `Error("canonical publication: …")`；成功：row `Loaded` + `publish_runtime_transition_views`。
6. disable：`unmount_inner`（`src/extension/lifecycle.rs:1215`）→ `retained_owner_stop`：**同步** `close_admission()` → provider `Stopping` → retained task 完成 revoke/drain/join/dispose；`OwnerFailure::{Revoke,Drain,Join,Stop,Dispose,MissingReceipt,MissingScope}` 是结构化残留。
7. B5 已验证（作为基线引用，不重跑即引用）：fmt、`check --all-targets`、定向 lib（lifecycle 57 / registry 53 / Zahir 23 / session 352）、`--lib` 21488 passed / 20 ignored、clippy 带 warnings（`src/extension/plugin_ops.rs:877` large `Err` 变体 + 两处既有 registry 测试 lint）。
8. **明确未做**：boot auto-activation、full failure-matrix acceptance（即本文件）。

---

## 3. B6.0 — source-backed 调用图与契约冻结 / Call-graph and Contract Freeze

B6.0 是**只读审计 + 决策记录**，不改生产代码。未通过前不得动 boot 路径。

### 3.1 必须审计的真实入口 / Required source audit

| 符号 / Symbol | 位置 / Location | 现状 / Current behavior |
|---|---|---|
| `ExtensionManager::ensure_loaded` | `src/extension/mod.rs:677` | fast path `cache_state.loaded`；否则取 `load_guard` → `load_all_locked` |
| `load_all_locked` | `src/extension/lifecycle.rs:307` | `discover_and_mount` → `views().after_transition()` → `cache_state.loaded = true` |
| `discover_and_mount` | `src/extension/lifecycle.rs:331` | `unmount_all` → 清 `runtime_owners` → settings → 阻塞扫描 → registry 重建 → 逐目录 admit + `declare_cold` / `mount_parsed`；失败按插件收集进 `failed` / `summary.errors` |
| `mount_inner` | `src/extension/lifecycle.rs:733` | Cold owner 存在 ⇒ `admit` + 返回 `Cold`（不 activation）；否则 parse → `eligible` ⇒ `declare_cold` |
| `set_plugin_enabled` | `src/extension/plugin_ops.rs:873` | 见 §2.2 第 3 条 |
| `activate_owner` | `src/extension/runtime_owner.rs:205` | Cold→Starting，retained task，单飞 |
| `mount_owner` | `src/extension/runtime_owner.rs`（`activate_owner` 之后） | 见 §2.2 第 5 条 |
| boot/ensure_loaded 调用点 | `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:677`、`tool_catalog_init.rs:162`、`src/gateway/execution_engine/run_loop/mod.rs:636`、`src/gateway/handlers/plugins/handlers/manage.rs:31`、`:205`、`src/builtin_tools/plugin_manage.rs:213`、`src/hub/reconcile.rs:78` | 全部只 `ensure_loaded().await` + warn，**没有任何 activation** |
| enable/disable 控制面 | `src/gateway/handlers/plugins/handlers/manage.rs:212`、`src/gateway/handlers/extensions/lifecycle.rs:96`、`src/bin/aleph-server/commands/plugins.rs:251`、`src/builtin_tools/plugin_manage.rs:279` | 全部收敛到 `set_plugin_enabled` |

### 3.2 已确证的结构性约束 / Proven structural constraints

- **C1（死锁，不是失败）**：`mount_owner` 自己取 `load_guard`（`tokio::select!` 中 `self.load_guard.lock()`）。任何在持有 `load_guard` 的调用栈里同步等待 activation 的写法都会**挂死**，而不是返回错误。因此 boot activation **不可能**放进 `discover_and_mount` / `load_all_locked` / `mount_inner`；唯一可行位置是 `load_all_locked` 返回、guard 释放之后，与 `set_plugin_enabled` 同形状（capture → 释放 guard → activate → 重取 → 复检）。
- **C2（fast path）**：`ensure_loaded` 在 `cache_state.loaded` 为真时直接返回。若 boot activation 放在 `load_all_locked` 之后，则必须明确 `loaded` 的置位时机与失败语义，否则"启动时漏激活"永远不会被第二次 `ensure_loaded` 修好。
- **C3（并发入口）**：启动期至少 4 个调用点会并发 `ensure_loaded`（agent_init、tool_catalog_init、run_loop、`plugins.list`）。activation pass 必须是 per-owner 单飞（`activate_owner` 的 `Starting` 分支已提供）且**不得**让每个调用方各自等待全量 mount 造成启动放大；具体等待策略在 B6.0 记录。
- **C4（测试脚手架边界）**：`RuntimeOwner.publication_barrier` / `.barrier` / `.stop_join_entered` 都是 `#[cfg(test)]`（`src/extension/runtime_owner.rs:64-80`）。竞态注入允许使用它们，但**不得**为了测试把生产字段打开；生产行为的断言必须另有一处只读真实状态（provider state、registry snapshot、scope 表、row status）的检查。
- **C5（计数可观察面）**：`discover_and_mount` 的 `mounted` 只收 `mount_parsed` 成功者，`summary.plugins_loaded = mounted.len()`；Cold 不计入。boot activation 成功后这个数字的语义必须显式定义（是否计入），否则 wire/doctor 报告会静默漂移。
- **C6（activation gate）**：`agent_init/mod.rs:677` 起 spawn 的 `activation_settled()` + `assess()` 读 row status；`Cold` 目前是 terminal。boot activation 引入 `Starting` 的可见窗口，gate 的 `waiting on ` 空列表文案问题（既有缺陷，`src/extension/activation_gate.rs`）在 B6 必须被**至少记录**，若 boot 期会观察到则一并修。

### 3.3 B6.0 必须产出的决策记录 / Decisions B6.0 must record

每条决策必须写成 "选项 / 证据 / 推荐 / 代价"，并**由用户确认**后才实施；主 Agent 不得静默选择。

**D-A：boot activation 的唯一生产入口**

- 选项 A（推荐）：`ensure_loaded` 内部两段式——`load_all_locked`（guard 内，仍是 Cold）→ 释放 guard → 一次 boot activation pass（仅对 row `Cold` 且 eligible 且 desired-enabled 的 exact owner 调 `activate_owner`）→ 返回。
- 选项 B：启动 wiring 里每个调用点各自做 activation pass（`agent_init` 专用函数）。
- 选项 C：新增独立 `activate_enabled_plugins()` 公共方法，由 `agent_init` 显式调用一次。
- 证据：C1 排除"guard 内激活"；C3 说明 4 个调用点并存；选项 A 让所有既有调用点自动获得一致语义，代价是 `ensure_loaded` 的契约从"load 完成"变成"load + 有界激活"，且必须处理 C2 的 fast path。选项 C 最小侵入但把"必须记得调用"变成新的隐性契约（形状 7：两端完整而中间没线）。
- 推荐 A，理由：唯一入口 + 既有调用点全覆盖；需接受 `ensure_loaded` 语义扩展与启动放大（T1）。

**D-B：多插件 boot activation 失败策略**（**必须由用户决定**）

- 选项 A（推荐）：per-plugin fail-closed——与现有 `discover_and_mount` 已采用的聚合语义一致（失败进 `failed` / `summary.errors`，其余插件继续）；失败插件 owner `ProviderState::Failed` + `OwnerFailure::*` + row `Error`，进程继续启动，activation pass 的失败不使 `ensure_loaded` 返回 `Err`。
- 选项 B：fail-fast——任一 enabled 插件激活失败即让启动失败/中止后续激活。
- 证据：现有 boot 对 `mount_parsed` 失败已是 per-plugin 收集（`load_all_locked` 只传播 discovery 级 `Err`），activation gate 也是 per-plugin 报告；选 B 需要**改造**既有聚合语义并决定 `ensure_loaded` 返回类型的影响面（4+ 个调用点只 `warn`）。
- 推荐 A（与真实启动契约一致、爆炸半径最小），但**这是 B6.0 必须显式记录的用户决策**，不得默认继承。

**D-C：`Cold` 在 B6 之后何时仍然可见**

需明确列出仍会产生 `Cold` 的路径（例如 activation context 未 attach、`mount()` 公共原语被直接调用、boot 失败后显式 disable/enable 循环、mixined/非 eligible 插件），并写进 B6.5 文档；不允许"boot 之后 Cold 不可能出现"这种未经验证的断言。

**D-D：boot activation 与 reload/replacement 的交互**

`discover_and_mount` 每次都会 `unmount_all` + 清 `runtime_owners`；`load_all()`（CLI/测试）与 `ensure_loaded` 都可能触发第二条 pass。必须冻结：第二条 pass 是否重新激活、旧 owner 的 stop 完成前是否允许新 owner Starting、以及 `cache_state.loaded` 何时被重置。

### 3.4 证据标准 / Evidence standard

- 任何 "已完成" 断言必须附：测试名 + 断言内容（效果，不是调用）或 命令行 exit code 与计数。
- **函数存在、类型存在、grep 命中、subagent 摘要、`cargo check` 通过，都不构成行为证据。**
- 每阶段必须同时给出四个语义：outcome、publication、resource、observer。缺一即视为该阶段未验收。

---

## 4. B6.1 — boot auto-activation 实现契约 / Boot Activation Contract

前置：B6.0 的 D-A/D-B 已被用户确认。

1. **触发条件（全部满足才激活）**：row 存在且 `status == PluginStatus::Cold`；`runtime_owner::eligible` 为真；`plugins.toml` desired state 为 enabled（经 `admit` 复核，不自造第二份 enabled 判定）；activation context 已 attach（`init_extension_manager` 装的 `Weak`）。
2. **禁止读侧触发**：catalog/describe/resolve/`plugins.list`/Zahir `describe`/doctor/observer 的**任何**路径都不得调用 `activate_owner`。验证方式：读侧调用前后断言 provider state、`runtime_owners` 条目、registry revision、VM 计数（loader `loaded_count()`/`is_loaded`）均不变。
3. **guard 协议**：capture exact owner（`Arc`）在 guard 内；`drop(load_guard)`；`activate_owner`；重新取 guard；用与 `set_plugin_enabled` **同一份**复检逻辑判定 `Mounted`/`Failed`。禁止两处各写一份复检（形状 1：同一事实的两份表述）——应抽成一个 crate-private helper，供 `plugin_ops` 与 boot pass 共用。
4. **owner incarnation / generation**：activation 只对 capture 到的 `Arc` 生效；`mount_owner` 内的 `owner_is_current` + `(Starting, generation)` 复检不得放松。boot pass 捕获的 owner 若在 `activate_owner` 返回前被 reload 替换 ⇒ 复检失败 ⇒ 结构化 `Failed`，不得报告 `Mounted`。
5. **`WasmMountIdentity` 与 canonical Ready commit**：沿用 `prepare_owner_batch` + `publish_plugin_batch_with_ready_commit`，不新增第二条发布路径。boot 与显式 enable 的发布必须是同一个函数。
6. **失败清理**：沿用 `mount_owner` 现有失败序列（`OwnerFailure::Publication` → 移除并 dispose scope → provider `Failed` → row `Error`），额外要求：boot pass 结束后，**每个** enabled 插件必须处于 `Mounted` 或结构化 `Failed` 之一，不允许 `Starting`/`Cold` 悬挂（`Cold` 仅在 D-C 列出的合法路径）。
7. **可观察面**：`LoadSummary` / `summary.plugins_loaded`（C5）、doctor、activation gate（C6）、`PluginRuntimeOutcome` 的 boot 报告，必须与真实状态一致；任何"报成功但没挂上"的 no-op 视为缺陷（形状 11）。
8. **幂等**：重复 `ensure_loaded`、并发 `ensure_loaded`、`ensure_loaded` 与 `set_plugin_enabled(true)` 并发，均不得产生第二个 owner、第二次 mount、或第二次 publication。

---

## 5. B6.2 — 完整 lifecycle 失败矩阵 / Full Lifecycle Failure Matrix

每一行都是**必须实现或必须显式声明为不可达**的契约，并对应一个命名测试。语义四列缺一不可。

| 阶段 / Phase | outcome | publication | resource | observer |
|---|---|---|---|---|
| discovery 解析失败 | row `Error`，插件不进入 eligible/Cold | 无 entry、revision 不变 | 无 VM、无 scope | 一次 transition 视图；`Invalidated` 不需要 |
| discovery 期间的 admit 拒绝（trust / disabled / not-enabled） | row `Blocked` / `Disabled`（含区分原因） | 无 entry | 无 VM | 视图更新一次 |
| Cold 声明 | `PluginStatus::Cold` / `PluginRuntimeOutcome::Cold` | 无 entry，revision 不变 | 无 VM | 视图显示 Cold（非 Ready） |
| activation admission（`admit` 在 `mount_owner` 内失败） | `Failed`，`error_phase = Reconcile` | 无 entry | provider `Failed`，无 scope | 视图更新；gate 看到非 terminal 之前必须先 Failed |
| provider `Cold → Starting` | `Starting` 可见窗口（长度有界） | 无 entry | 无 VM | 观察者不得把 `Starting` 读成 Ready |
| mount 失败（`mount_parsed` Err） | `Failed` + `MountError` 文本 | 无 entry | 已产生的 scope 必须 dispose；dispose 失败 ⇒ `OwnerFailure::Dispose` 残留 | 视图更新，row `Error` |
| binding / export 校验失败（`bind_wasm`） | `Failed`（`PluginPublicationError::InvalidEntry`） | 无 entry | dispose scope | 同上 |
| owner / provider / mount 复检失败（stale incarnation / generation） | `Failed`（"mount completion is stale…"） | 无 entry | **保留**已挂载资源并标记 residual，不静默丢弃 | 视图更新 |
| publication commit 失败 | `Failed`（row `Error("canonical publication: …")`） | 无 entry，revision 不递增 | 移除并 dispose scope，失败则 residual | 视图更新一次 |
| Ready 成功 | row `Loaded`，`Mounted` | 完整 batch + receipt，revision 单调 +1 | scope 持有，`identity`/`receipt` 齐备 | 一次广播 + 视图 |
| dispatch（canonical，成功） | 返回 guest payload | 不变 | 持 permit，结束后释放 | 无 |
| dispatch 期间 stale（owner 被替换/generation 变） | 结构化 stale 错误，绝不 fallback 到 name 解析 | 不变 | permit 释放 | 无 |
| disable（正常） | `Unmounted` | entry revoke，revision +1 | drain → join → dispose | 广播 revoke + 视图 |
| disable（revoke 失败） | `Failed` + `OwnerFailure::Revoke` | 旧 entry 状态必须如实报告（不得声称已撤回） | 不提前 dispose | 视图如实 |
| drain 超时/失败 | `Failed` + `OwnerFailure::Drain` | 已 revoke | **不** dispose（在途调用仍持 permit） | 视图如实 |
| join 失败 | `Failed` + `OwnerFailure::Join` | 已 revoke | 保留 residual | 视图如实 |
| unload / dispose 部分失败 | `Failed` + `OwnerFailure::Dispose(report)` | 不变 | 失败步的残留保留在 owner | 视图如实 |
| replacement / reload | 新 generation + 新 mount identity + 新 receipt；旧 lease/handler/binding fail-closed | revision +1（新一轮），旧 handle 只 no-op/stale | 旧 scope 先 dispose，再挂新的 | 观察者看到 replace，不复活旧 entry |
| stale completion（旧激活任务晚到） | no-op / stale 错误 | 不得删除 replacement entry | 旧资源由自己的 scope 处理 | 不得广播旧状态 |

**硬约束**：`Mounted`/`Loaded`/`Ready` 三者任一出现时，另外两者必须已成立；不允许任何中间状态被投影成 Ready（形状 8：fail-closed 的答案被当成值消费）。

---

## 6. B6.3 — 并发 / 替换 / observer 验收 / Concurrency, Replacement, Observer Acceptance

实施方式：真实 WASM guest fixture（`write_barrier_guest` + `PAYLOAD_A`）、真实 `RuntimeOwner`、真实 `ToolHandlerRegistry`、真实 provider 迁移、`tokio::sync::{Barrier, Notify, oneshot}` 注入时序。**禁止 `sleep` 排序；禁止 mock-only success；禁止只统计调用次数。**

| 编号 | 场景 | 必须证明 |
|---|---|---|
| X1 | boot activation race（两个 `ensure_loaded` 并发 + 期间 `plugins.list`） | 只有一个 owner、一次 mount、一次 publication；读侧不触发 activation；最终 row 状态唯一 |
| X2 | boot activation 与 `set_plugin_enabled(true)` 并发 | 同一 owner 单飞；不出现二次 `Cold→Starting`；两次调用都得到与真实状态一致的结果 |
| X3 | boot activation 与 disable 并发 | disable 先 fence：`close_admission` 后 activation 返回失败且不留 entry；或 activation 先完成则 disable 完整 revoke+drain+dispose；无中间态 |
| X4 | in-flight guest call + disable | drain 等待在途 permit；dispose 不早于调用结束；调用返回 stale 或成功但绝不在已 dispose 的 scope 上执行 |
| X5 | same-name replacement（reload 期间 enable） | 新 entry 可用；旧 disposer / late completion 不撤回新 entry；旧 lease 调用返回 stale |
| X6 | 旧 owner / provider generation / mount identity / binding / lease 的 fail-closed | 每一维度单独构造陈旧值，全部拒绝，且错误可区分（`StaleOwner` / `StaleProvider` / `StaleMount` / `Unknown`） |
| X7 | observer lag / resync | Zahir `Lagged` ⇒ `describe` + `Invalidated`；resync 后不复活旧 entry；不承诺 durable replay；`SessionEventStore` 仍是唯一 session durability |
| X8 | publication barrier 注入（`#[cfg(test)]`） | 在 Ready commit 前暂停，断言此刻无 callable entry、无 `Loaded` row、无 receipt；释放后才三者齐备（C4：另有一处只读真实状态断言） |
| X9 | boot 期间 stop fence（`stop_requested` 先于 `load_guard`） | activation 在取 guard 前退出，provider 不进入 `Ready`，row 不 `Loaded` |

---

## 7. B6.4 — 真实边界 QA 与验证矩阵 / Real Boundary QA and Validation Matrix

### 7.1 内存门（每条 cargo 命令前单独执行）/ Memory gate before every cargo command

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有 self-test exit 0 且探针 exit 0 且 `available_bytes >= 4294967296` 才允许 cargo；cargo **串行**执行，不并发共享 target。

### 7.2 验证矩阵 / Validation matrix（按实际 diff 选择，逐条记录）

| 命令 | 目的 | 记录字段 |
|---|---|---|
| `cargo fmt --all --check` | 格式 | exit / 输出 |
| `cargo test -p alephcore --lib extension::lifecycle::` | boot + lifecycle 矩阵 | passed / failed / ignored |
| `cargo test -p alephcore --lib extension::plugin_ops::` | enable 面与 outcome | 同上 |
| `cargo test -p alephcore --lib extension::runtime_owner::` | owner/generation/mount | 同上 |
| `cargo test -p alephcore --lib tools::registry::` | publication/revoke/revision | 同上 |
| `cargo test -p alephcore --lib capability::zahir_facade::` | observer lag/resync | 同上 |
| `cargo test -p alephcore --lib` | 全量 lib 回归 | passed / failed / ignored |
| `cargo check -p alephcore -p aleph-slot-api --all-targets` | 编译全 target | exit + warnings |
| `cargo clippy --workspace --all-targets` | lint | exit + warnings（含既有 `plugin_ops.rs:877`） |

规则：filter 命中 0 tests 不算通过；旧日志复用不算通过；未执行写 `UNRUN`；BLOCKED 必须写明原因（内存门失败 / fixture 缺失 / 工具链）。

### 7.3 真实边界 / What "real boundary" means here

- 真实启动：经由 `ensure_loaded` 的真实调用点形状（CI 中可用 in-crate harness 复刻同一函数与同一 guard 序列，但不得绕过 `ensure_loaded` 直接调 `activate_owner`）。
- 真实 WASM：`write_cold_tool_wasm_fixture` 系列的 guest，执行真实 export，返回 `PAYLOAD_A` 级确定 payload。
- 真实 publication：`ToolHandlerRegistry::publish_plugin_batch_with_ready_commit` / `revoke`，revision 从真实 snapshot 读。
- 真实 observer：`ZahirFacade`（subscribe-before-snapshot）+ registry broadcast。
- 不足以作为证据：`#[cfg(test)]` 断点本身；`MockProvider`；只断言 `Result::is_ok()`；只断言 row 字段而不看 registry/provider/scope。

---

## 8. B6.5 — 文档、验收与报告 / Docs, Acceptance, Report

1. 更新 `docs/reference/FEATURE_LOCATOR.md` §3.5e（或新增 §3.5f）：写清 boot activation 的唯一入口、D-B 的失败策略、仍可见 `Cold` 的路径（D-C）、`Loaded/Ready/Mounted` 三者的唯一提交点。
2. 更新 `docs/reference/ARCHITECTURE.md` 与 `docs/reference/TOOL_SYSTEM.md` 的 B5 段落后继段：把 "B6 full failure-matrix acceptance remains deferred" 改成已验收事实（含验证计数与 warnings）。
3. 验收报告必须包含：修改文件清单与真实调用图；每阶段四语义矩阵的落地位置（测试名）；每条 cargo 的 memory gate + exit + 计数 + warnings；failed/ignored/UNRUN 清单；残余风险；未提交/未推送状态。
4. 报告必须显式说明 **没有做什么**（见 §10）。

---

## 9. 允许修改范围、禁止事项、停止条件 / Allowed Scope, Prohibitions, Stop Conditions

### 9.1 允许修改 / Allowed

- `src/extension/mod.rs`（`ensure_loaded` 两段式）、`src/extension/lifecycle.rs`（boot pass 支撑、失败矩阵、测试）、`src/extension/plugin_ops.rs`（抽出共用复检 helper、outcome 语义）、`src/extension/runtime_owner.rs`（仅在确有 fenc 缺口时；优先不改）、`src/extension/plugin_state.rs`（仅当 outcome 枚举需要新语义）。
- `src/extension/activation_gate.rs`（仅 C6 的文案/等待缺陷）。
- 启动 wiring：`src/bin/aleph-server/commands/start/builder/agent_init/{mod.rs,tool_catalog_init.rs}` 仅在 D-A 选择选项 B/C 时。
- 测试与 fixture：`src/extension/lifecycle.rs` 内测试、`src/extension/runtime/wasm/tests/guest_fixture*`（仅新增 guest，不改既有语义）。
- 文档：`docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,TOOL_SYSTEM}.md`、本目录 research 文档。

### 9.2 禁止 / Prohibited

- 修改 `src/harness/`（12 文件棘轮）。
- 新建第二 callable registry、第二 enabled 判定/durable store、第二 actual-state authority、独立 disposer map。
- 引入后台重试调度器 / universal scheduler；引入 activation proxy 让 canonical 首次调用隐式启动 Cold（那是独立架构决策）。
- 读侧触发 activation；用 metadata/catalog/role/`TurnContext`/name fallback 合成 authority。
- 把 `#[cfg(test)]` barrier 打开成生产字段；用 sleep 或 mock-only 通过验收。
- 为了测试通过而放宽 `eligible`、跳过 admission、跳过 drain、伪造 revision。
- 自动 commit / push / reset / rebase / 覆盖共享工作树。

### 9.3 停止条件 / Stop and report

- B6.0 的 D-B（fail-fast vs per-plugin）未获用户确认——**不要**先实现其中一种。
- 需要改变 B5 已冻结的 Cold/Ready/stop 权限语义，或需要新增 activation proxy 才能达成 S1。
- 只能靠绕过既有 gate 或伪造 revision 才能让测试通过。
- 内存门失败、真实 WASM fixture 不可用、共享 writer 冲突、连续三次无实质进展。
- 发现 C1（死锁）之外的第二个结构性阻塞（例如 `ensure_loaded` 语义扩展会破坏既有调用点契约）——先报告，不自行扩大 diff。

---

## 10. B7 及以后 deferred / Deferred Beyond B6

MCP lazy semantics、`EffectClaim` producer、automatic Safe Replay、universal durable scheduler、其余 Capability kinds（Gateway/Terminal/Memory backend）、durable external-effect exactly-once、`src/harness/` 改造、eligible 面扩大（mixed/static 插件转 lazy）、activation proxy，以及为达成"禁用时连代码都不在内存"强语义所需的**进程 / 动态加载边界**（sidecar / dylib / 子进程）。

**本文件明确不做 / Explicitly not done here:** 不改任何 Rust 代码；不运行 cargo；不 commit/push/reset/rebase；不修改既有 reference 文档；不替用户决定 D-B。

*Last updated: 2026-10-23*
