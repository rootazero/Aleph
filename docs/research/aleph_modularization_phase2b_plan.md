# Aleph 模块化第二阶段 B 计划
# Phase 2 B Plan — Canonical Plugin Producer and Long-Lived Entry Fencing

## 1. 目标与边界 / Goal and Boundary

本工作包在 Phase 2 A 的 RuntimeOwner、Cold discovery、sealed admission、stop fencing 基础上，完成 **B：canonical plugin producer + per-entry provider fencing + long-lived canonical projection**。

最终目标是：经过真实验证并已 Ready 的 tool-only WASM plugin instance，能够以一组带 provider identity、owner generation、mount identity 和 registry revision 的 canonical `ToolHandlerRegistry` entry 进入既有 ToolService 调用面；实例停止、替换、reload 或 generation 变化时，旧 entry/handler/lease 必须 fail-closed 并被准确撤回，长期持有的 canonical facade/catalog/observer 能看到当前权威状态。

本阶段不重新实现 A 的 RuntimeOwner，不把 metadata/catalog 当 callable registry，不把 B 的局部接线说成所有插件能力完成。

## 2. 当前起点 / Starting State

- 工作目录：`.`。
- A 的实现仍是共享工作区未提交变更；新 session 必须先读取 `git status --short` 和实际 diff，不能 reset、checkout 旧提交、覆盖共享修改或自动提交/推送。
- `RuntimeOwner` 已负责 tool-only WASM 的 Cold → Starting → Ready、single-flight activation、generation、WASM identity、admission permits、exact scope disposer 和 stop residual。
- `PluginActivationAuthority` 只能从 sealed canonical gate verdict 派生；不能从 plugin id、角色、`TurnContext`、metadata 或 direct RPC 合成。
- compatibility inner plugin 路径当前在没有真实 canonical identity 时 fail-closed；这是当前 B 的入口证据，不得绕过它。
- `ToolHandlerRegistry` 是 canonical callable authority；`PluginRegistry`、active tool metadata、catalog、diagnostic、provider broadcast 都是 projection 或 notification。
- `reconcile_plugin_tools()` 当前应保持 producer-not-mounted 的 fail-closed 行为，直到 B 的契约、事务和测试全部冻结。

## 3. B0：独立 gate 与架构决策 / Independent Gate

在修改 canonical producer 前，必须完成并保存一份短 plan review，至少回答：

1. 当前 `ToolHandlerRegistry`、`ProviderCallHandler`、`RegistryEntry`、`ToolCallIdentity`、`BackendLease`、`ZahirFacade` 和 `ExtensionManager` 之间的真实调用图。
2. producer 的唯一输入是什么：必须来自已验证 manifest/adapter declaration、当前 Ready `RuntimeOwner`、实际 mounted `WasmMountIdentity` 和真实 handler binding。
3. 一个 canonical entry 是否代表“当前已 Ready 的可执行 handler”，还是代表“会在调用时激活 Cold owner 的 proxy”。
4. 默认推荐：Cold 不进入可执行 canonical registry，producer 只发布 Ready 且 identity-fenced 的 handler；如果用户需要模型第一次调用即可激活 Cold，必须单独审定 activation proxy 的权限、visibility、confirmation、lease 和 stop 语义，不能暗中改变 A 的 Cold 契约。
5. 注册、替换、撤回、disposer、provider generation、owner generation、metadata revision、canonical registry revision 的权威和事务边界。
6. 失败顺序：producer 注册失败、同名冲突、owner stop 失败、旧 disposer 晚到、provider generation 不匹配时分别保留什么状态。
7. long-lived observer 如何通过真实 broadcast/notification 得到变化并从当前 snapshot 重建；不得承诺 durable replay，也不得虚增 revision。

B0 未通过前，不修改 `src/tools/registry.rs` 的 producer 成功路径。

## 4. 实施 DAG / Implementation DAG

```text
B0 plan review + canonical contract freeze
  → B1 producer input and entry lease contract
  → B2 atomic register/replace/revoke with provider and owner fencing
  → B3 real RuntimeOwner lifecycle integration
  → B4 canonical dispatch and sealed authority integration
  → B5 long-lived projections and observer recovery
  → B6 real WASM/effect/lease/concurrency acceptance
  → GB/B final report
```

### B1：producer 契约 / Producer Contract

允许优先修改：

- `src/tools/registry.rs`
- `src/capability/{backend,facade,zahir_facade}.rs`
- `src/extension/{runtime_owner,lifecycle,plugin_ops,projection}.rs`
- 必要的 `src/tools/descriptor.rs` 和现有 registry 类型

要求：

- 每个 producer entry 携带 plugin id、mount identity、provider generation、owner generation、handler identity 和真实 canonical descriptor。
- entry 的 executable admission 必须要求 provider `Ready`、owner current、generation 相等、WASM binding 当前且 desired/trust/visibility 仍允许。
- 不复制第二份 actual state、enabled store、callable registry 或 disposer map。
- `ToolCallIdentity` 必须由 canonical descriptor 派生；不能用 plugin metadata 名称代替。
- B 的 lease/entry 不能跨 provider restart 或同 id replacement 继续有效。

### B2：原子注册与撤回 / Atomic Publish and Revoke

- Ready transition 与 canonical entry publish 的顺序必须冻结：entry 只有在 scope、WASM identity、owner generation 和 callable admission 全部完成后才可见。
- stop/reload 先关闭新 admission，再撤回旧 canonical entry，等待现有 entry call 的安全边界，最后 dispose exact scope。
- register/replace/unregister 使用现有 registry revision 和 registration handle；旧 handle 不能撤销新 entry。
- 同名冲突、partial registration、disposer failure 必须 fail-closed，并保留结构化 diagnostic。
- 旧 entry 晚到的 completion/disposer 只能 no-op 或报告 stale，不能删除 replacement entry。

### B3：RuntimeOwner 接线 / Lifecycle Integration

- `RuntimeOwner` 仍是实际 WASM 生命周期的唯一 owner；producer 只消费它已证明的 Ready snapshot 和 binding。
- A 的 Cold discovery 不得因为 catalog、describe、resolve、projection 或 observer 查询而 activation。
- activation 成功后 producer publish；activation failure 不得留下 canonical entry。
- stop 成功撤回 entry、dispose scope、发布 Stopped/Disabled；stop residual 保留 Failed/residual 和旧资源，不伪报已撤回。
- reload/replacement 重新分配 generation 和 mount identity，禁止复用旧 lease。

### B4：canonical dispatch / Sealed Dispatch Integration

- 真实 canonical entry 通过既有 `ToolHandlerRegistry`/`ScopedToolService` permission、visibility、trust、confirmation 和 actor gates。
- 只有在 gate scope 内才能创建 `PluginActivationAuthority`；authority 绑定 canonical name、descriptor identity、input、call id、caller 和 actor。
- compatibility inner route 没有真实 canonical entry 时继续拒绝，不增加 name-based fallback。
- direct gateway RPC 不得伪造 canonical entry 或 authority；它只能调用已 mounted/Ready 的 owner path。
- 真实 canonical handler 调用后必须重新验证 provider/owner generation、mount identity 和 current entry，再接受 guest output。

### B5：long-lived projection / Observer Contract

- canonical registry snapshot、facade、catalog、MCP/tool observer 使用真实 registry revision；不得把 provider generation 或 metadata revision 改名为 canonical revision。
- registry broadcast 是 live notification，丢事件后从当前 canonical snapshot 重建；不添加假 replay。
- provider broadcast 只做通知，不能作为 durable recovery source；durable recovery 仍由既有 session event store 负责。
- observer 在不重建持有对象的情况下，必须看见 entry publish、撤回、replacement 和 stale 状态。
- 一个 plugin stop 不能撤回 builtin 或其他 plugin 的 entry。

## 5. 必须验证的真实效果 / Required Effect Evidence

优先复用现有真实 WASM fixture、`WasmMountIdentity` 测试、`EffectScope` disposer 和 barrier/oneshot，不使用 sleep 排序或 mock-only success。

至少覆盖：

1. Cold discovery：重复 list/catalog/describe/resolve 不创建 VM、不发布 canonical executable entry、不递增 provider/registry generation。
2. Ready producer：真实 WASM mount 完成后发布一个带完整 identity/generation 的 canonical entry。
3. canonical call：通过真实 permission/visibility/confirmation gate，进入 `PluginActivationAuthority`，执行真实 guest handler 并返回确定 payload。
4. old lease rejection：stop、reload、replacement 后，旧 entry、旧 handler、旧 binding 和旧 lease 全部 fail-closed。
5. same-name replacement：新 entry 可用，旧 disposer/late completion 不会撤回新 entry。
6. concurrent calls：同一 provider generation 下并发调用正确共享 admission/lifetime；stop 不会在途调用仍使用资源时提前 unload。
7. producer failure：同名冲突、注册部分失败、disposer error、provider mismatch 都产生可查询失败状态，不报成功。
8. observer：已存在的 facade/catalog/observer 看见 publish/revoke/replace；lag/reconnect 从当前 snapshot 恢复，不复活旧 entry。
9. mixed/static regression：mixed WASM、MCP、static 继续使用各自原语义；本阶段只扩大明确批准的 Tool producer。
10. B boundary：未 Ready 或未 canonical producer 的 plugin 仍返回 producer-not-mounted/permission denied，不通过 metadata 旁路执行。

## 6. 资源与验证纪律 / Resource and Verification Discipline

每一条 cargo 命令之前都必须单独运行：

```bash
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

只有 self-test exit 0、探针 exit 0 且 `available_bytes >= 4294967296` 才允许运行 cargo。共享 target 不并发。不得把 filter 0 tests、旧日志、只编译成功或 subagent 摘要当成测试通过。

候选验证集按实际 diff 选择：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib tools::registry::
cargo test -p alephcore --lib capability::zahir_facade::
cargo test -p alephcore --lib extension::runtime::wasm::
cargo test -p alephcore --lib extension::lifecycle::
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
cargo clippy --workspace --all-targets
```

wire/shared 变化必须补跑对应 shared tests；缺少真实 fixture 或工具链时记录 BLOCKED/UNRUN，不用 mock 替代。

## 7. 禁止事项与停止条件 / Prohibitions and Stop Conditions

禁止：

- 修改 `src/harness/`。
- 把全局 `aleph/tools` provider 改成单插件 provider。
- 新建第二 callable registry、第二 durable enabled store、第二 actual-state authority 或独立 disposer map。
- MCP lazy、sidecar、dylib、scheduler、Safe Replay、EffectClaim producer、跨 Capability kinds 扩展。
- 读侧触发 activation，或用 metadata/catalog/role/TurnContext 合成 authority。
- 在 B0 未冻结前把 Cold 暴露成可执行 canonical entry。
- 将 provider broadcast 当 durable recovery、虚增 canonical revision、吞掉 partial failure。
- reset、覆盖共享工作区、自动 commit、自动 push。

必须停止并报告证据的情况：

- 需要改变 A 已冻结的 Cold/Ready/stop 权限语义。
- 需要引入 activation proxy 才能完成首次模型调用但尚未获得该架构决策。
- 无法证明旧 entry/disposer/late completion 不会影响新 entry。
- 只能通过绕过既有 permission/visibility/trust/confirmation 或伪造 revision 使测试通过。
- 内存门失败、真实 WASM fixture 不可用、共享 writer 冲突或连续三次无实质进展。



## B0 Review Record / B0 评审记录

**Target:** `docs/research/aleph_modularization_phase2b_plan.md` (固定评审目标)。

### Scope and current evidence

- 现有 canonical 调用链：`ScopedToolService` 解析 `ToolHandlerRegistry` entry，经 permission/visibility/trust/confirmation/actor gates 产生 `ToolCallIdentity`，再调用由 `ProviderCallHandler(AdmissionHandler(raw))` 包装的 handler（源码证据：`src/tools/scoped/dispatch.rs:250-347,1012-1052`、`src/tools/registry.rs:677-706`）。
- 现有 compatibility 链：`BuiltinToolRegistry` → `ExtensionManager::call_plugin_tool_with_authority` → `RuntimeOwner`/loader；`reconcile_plugin_tools()` 仍返回 `plugin producer reconciliation is not mounted`，`PluginCanonicalHandler` 尚无生产调用者（源码证据：`src/executor/builtin_registry/registry/tool_registry_impl.rs:55-78`、`src/extension/plugin_ops.rs:11-89`、`src/tools/registry.rs:759-773`、`src/extension/canonical_handler.rs:15-68`）。
- 现有 registry registration handle 绑定 `(name, revision)`；旧 handle 对 replacement 只 no-op，不会撤回新 entry（`src/tools/registry.rs:278-347,538-674`）。`ZahirFacade` 先订阅 live notification 再取 snapshot；broadcast 不是 durable recovery（`src/capability/zahir_facade.rs:406-435`、`src/capability/versioned_registry.rs:121-136,615-635`）。
- 已冻结范围：Ready-only canonical executable entry、Cold 不进入 executable registry、复用现有 registry/gates/lease 原语、原结构、两波交付；本轮不实现 activation proxy、不修改 `src/harness/`。

### B0 architecture decision summary

1. **Ready-only:** canonical producer 只接受真实 Ready `RuntimeOwner`、当前 `WasmMountIdentity`、provider/owner generation 和真实 handler binding；Cold 只能被既有受控 activation 路径启动，不能由 canonical registry 首次调用隐式启动。
2. **Single callable authority:** `ToolHandlerRegistry` 仍是唯一 callable registry；`RegistryEntry`/`RegistrationHandle` 是 entry/revision 权威，`ProviderHandle` 是 provider state/generation 权威，`RuntimeOwner` 是 owner generation/mount/effect scope 权威，`ZahirFacade` 只做 snapshot/notification projection。
3. **Fail-closed:** 同名冲突零提交；多 entry 发布必须全有或全无；任一 provider/owner/mount/descriptor mismatch 拒绝；旧 handle、旧 lease、late completion/disposer 只能 stale/no-op；disposer residual 不得伪报 clean stop。
4. **Observer:** subscribe-first + current snapshot；`event.revision <= snapshot.revision` 丢弃；lag、gap、close 重新从当前 snapshot 建立，不把 broadcast 当 replay 或 durable recovery。
5. **Delivery:** 第一波只实现并验收 B1/B2 entry contract 与 atomic register/replace/revoke；第二波接 RuntimeOwner、sealed dispatch、observer 和 GA/GB。

### Scope record

`feature answers: Ready-only canonical producer and Cold exclusion (user request); two-wave delivery (D0); structure: original arrangement (D1); accepted scope: B1/B2 first wave, then B3-B6, preserving all listed contracts, gates, real fixtures and failure semantics; pending remedies: R1-R6 below.`

## Decision ledger

### R1: Ready 与 canonical registry publish 的唯一线性化点

Finding: 1, P0, confidence 9/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:27-32,68-72`。

Plan baseline: plan requires a canonical entry to be visible only after scope/WASM identity/owner generation/callable admission are complete, but does not define one atomic commit point.

Runtime evidence: `src/tools/registry.rs:1286-1306` permits registry/provider transitions to occur independently; `src/tools/registry.rs:677-706` performs provider readiness admission separately. Existing code therefore cannot be treated as an atomic Ready+entry publish.

Comparison grid:

| Contract | Current | A: composed publish transaction (recommended) | B: staged fail-closed publish |
|---|---|---|---|
| Ready/entry visibility | independent transitions; not sufficient | producer prepares identity-fenced batch while owner is Starting, then one coordinator commit makes Ready + canonical snapshot visible as one public operation | provider may enter Ready before entry snapshot; entry remains non-executable until all checks pass, with explicit transient state and rollback |
| Registry authority | `ToolHandlerRegistry` | unchanged; add a narrow batch publish/commit seam, no second registry | unchanged; use existing single-entry APIs plus fencing guards |
| Failure behavior | unspecified | commit failure leaves no public Ready or entry; residual is structured | any failed second step leaves owner Failed/residual and entry revoked; transient observer states must be documented |
| Proof burden | missing | barrier test proves no Ready-only or entry-only public window | tests must prove every transitional snapshot is non-executable and never reported Ready |
| Scope | pending | B1/B2 contract + B3 lifecycle integration | B1/B2 contract + B3 lifecycle integration |

Question D2:
D2 — Ready 与 entry 如何原子发布？
Project/branch/task: `main` / Phase 2B canonical producer B0。
ELI10: 发布一个插件像同时开门和挂上门牌。现在两件事由不同对象完成，可能出现门已经显示“营业”但门牌还没挂好，或门牌存在但服务还没准备好。这个决定规定外部观察者和调用者能看到的唯一提交点。
Stakes if we pick wrong: observers may see a false Ready state, callers may resolve an entry that can never execute, and replacement cleanup may race with a half-published instance.
Recommendation: A because it gives one public commit boundary and makes the required no-partial-publish proof local to the producer transaction。
Completeness: A=9/10, B=7/10
Pros / cons:
A) 组合 publish transaction (recommended)
  ✅ `Ready` and the canonical snapshot become visible through one coordinator boundary, so the producer can prove that no caller sees a callable half-instance.
  ❌ Requires a narrow batch-commit seam across RuntimeOwner and the existing registry, increasing B1/B2 integration work and barrier-test surface.
B) staged fail-closed publish
  ✅ Reuses more existing single-entry APIs and keeps the first code change smaller while every transitional call still rejects closed admission.
  ❌ Observers must understand a transitional state and the plan carries a harder proof that no Ready-only or entry-only window leaks through projections.
Net: choose whether correctness is proved by one public commit boundary or by fencing every intermediate state。
Header: Publish boundary
Options:
A) 组合 publish transaction (recommended)
The producer stages all identity checks and registrations, then commits owner Ready and the canonical registry snapshot as one public operation. A commit failure publishes neither and retains structured residual state when cleanup cannot complete. (human: ~2 days / CC: ~30 min)
B) staged fail-closed publish
The producer may pass through an internal transitional state, but no caller or observer may treat it as Ready/executable until both sides commit. Any failure revokes the entry, closes admission and leaves Failed/residual state. (human: ~1.5 days / CC: ~25 min)

State: approved
Actual answer: A) 组合 publish transaction (recommended), user answer to D2
Accepted scope: Add a narrow coordinator transaction in B1-B3. Producer stages identity checks and the full entry batch while owner remains non-public; one commit boundary makes owner Ready and canonical snapshot visible together. Commit failure publishes neither and leaves structured residual state if cleanup fails.
History: none

### R2: 插件多 entry 的事务单位

Finding: 2, P0, confidence 9/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:68-72`。

Plan baseline: the plan requires partial registration to fail closed but does not define the transaction unit, preflight order, or rollback owner for a plugin exporting multiple tools.

Runtime evidence: production `ToolHandlerRegistry` exposes single-entry `register`/`replace` operations (`src/tools/registry.rs:538-674`); batch publication exists only in test helpers (`src/tools/registry.rs:897-1004`).

Comparison grid:

| Contract | Current | A: plugin batch snapshot swap (recommended) | B: sequential register with compensation |
|---|---|---|---|
| Transaction unit | unspecified | one plugin publication, one prepared set, one snapshot commit and one canonical revision event | one entry at a time, with cleanup on later failure |
| Conflict handling | unspecified | preflight every name, descriptor and generation before mutation; any conflict means zero mutation | detect conflicts as encountered and compensate prior mutations |
| Partial failure | unspecified | old snapshot remains unchanged; residual diagnostics belong to RuntimeOwner | compensation may fail after earlier mutations; requires rollback visibility and more states |
| Registry authority | `ToolHandlerRegistry` | unchanged; narrow batch transaction over existing registry | unchanged; existing single-entry methods reused |
| Proof burden | missing | one barrier test for all-or-nothing publication and one revision/event assertion | per-step ordering, compensation and disposer-failure matrix |

Question D3:
D3 — 一个插件的多个 canonical entry 如何提交？
Project/branch/task: `main` / Phase 2B B1/B2。
ELI10: 一个插件可能一次提供多个工具。问题是要不要像一次打包提交一样全部挂上，还是挂一个、再挂一个，出错时再拆掉前面已经挂好的。前者让外界只看到完整版本，后者更容易复用旧 API 但失败路径更多。
Stakes if we pick wrong: users can observe half a plugin, and a failed export can leave callable handlers or cleanup that belongs to the wrong instance.
Recommendation: A because preflight plus one snapshot swap gives a simple all-or-nothing invariant and reuses the existing registry as the only authority。
Completeness: A=9/10, B=6/10
Pros / cons:
A) plugin batch snapshot swap (recommended)
  ✅ All names, descriptors and generations are checked before mutation, so a conflict leaves the old snapshot intact and observers receive one coherent revision.
  ❌ Requires a narrow batch transaction API and a plugin-level publication object, adding integration work to the first wave.
B) sequential register with compensation
  ✅ Uses current single-entry operations directly and may reduce the first implementation diff if plugins expose only one tool.
  ❌ Compensation can itself fail, leaving partial state and forcing callers to distinguish rollback success, residual resources and a still-visible old instance.
Net: choose whether plugin publication is a single coherent registry state or a sequence of compensating mutations。
Header: Plugin publish unit
Options:
A) plugin batch snapshot swap (recommended)
Preflight every entry and commit the complete plugin set as one registry transaction and one canonical revision. Any conflict or validation failure leaves the prior snapshot unchanged; cleanup failures become structured RuntimeOwner residual state. (human: ~2 days / CC: ~35 min)
B) sequential register with compensation
Register entries one by one using existing APIs, then compensate already-published entries if a later entry fails. The implementation must expose rollback failure and never report a partial plugin as Ready. (human: ~2 days / CC: ~30 min)

State: approved
Actual answer: A) plugin batch snapshot swap (recommended), user answer to D3
Accepted scope: Add plugin-level preflight and one snapshot transaction. All entries are validated before mutation; one canonical revision/event publishes the complete set. Any conflict leaves the previous snapshot unchanged; residual cleanup is owned by RuntimeOwner and reported structurally.
History: none

### R3: stop admission、provider、revoke、drain、dispose 的顺序

Finding: 3, P0, confidence 9/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:68-80`。

Plan baseline: the plan says close new admission, revoke the entry, wait for existing calls, then dispose the exact scope, but leaves the ordering between owner admission, provider state, registry revoke and permit drain unspecified.

Runtime evidence: `src/extension/runtime_owner.rs:47-65` closes owner admission through an atomic flag and checks it around permit acquisition; `src/capability/provider.rs:278-282` does not drain in-flight work. Therefore provider transition alone cannot replace the owner admission barrier.

Comparison grid:

| Contract | Current | A: owner-first stop barrier (recommended) | B: provider-first stop barrier |
|---|---|---|---|
| First action | ordering unspecified | close owner admission under stop barrier | provider enters Stopping first |
| New calls | admission flag rejects new permits before revoke | deterministic owner fence; provider then rejects new leases | callers can race between provider transition and owner permit check |
| Registry | unspecified | atomically revoke exact entry batch after owner close | revoke after provider state change |
| In-flight calls | unspecified | drain owner permits before scope dispose | provider state is expected to gate calls, but does not drain them |
| Failure | unspecified | never reopen admission; dispose failure leaves Failed/residual | can leave an ambiguous provider/owner split if later revoke or drain fails |

Question D4:
D4 — stop/reload 的 admission 顺序如何冻结？
Project/branch/task: `main` / Phase 2B RuntimeOwner integration。
ELI10: 停止插件时，先要禁止新客人进门，再把门牌摘掉，等屋里的人出来，最后拆房子。问题是“禁止进门”由 owner 还是 provider 先做，以及什么时候从 registry 中撤回 entry。
Stakes if we pick wrong: a new call can obtain a permit while the WASM scope is being disposed, causing use-after-dispose behavior or a replacement race.
Recommendation: A because the owner admission flag is the only existing primitive that directly fences in-flight permits and it can be followed by exact registry revoke and drain.
Completeness: A=9/10, B=5/10
Pros / cons:
A) owner-first stop barrier (recommended)
  ✅ Close owner admission first, then transition provider to Stopping, revoke the exact entry batch, drain permits, and dispose the exact scope without reopening admission.
  ❌ Requires RuntimeOwner and provider transition to participate in one ordered stop protocol and adds barrier assertions to lifecycle tests.
B) provider-first stop barrier
  ✅ Reuses provider lifecycle state as the first visible stop signal and may simplify external status reporting.
  ❌ ProviderHandle does not drain owner permits, so a call can cross the provider transition while owner shutdown or registry revoke is still pending.
Net: choose the barrier whose first operation controls the actual in-flight owner permits。
Header: Stop order
Options:
A) owner-first stop barrier (recommended)
Close owner admission, transition provider to Stopping, atomically revoke the exact entry batch, drain existing permits, then dispose the exact scope. Any failure leaves admission closed and reports Failed/residual. (human: ~1.5 days / CC: ~25 min)
B) provider-first stop barrier
Transition provider to Stopping, then close owner admission, revoke entries, drain permits and dispose scope. The implementation must prove that no owner permit can be acquired during the provider-to-owner gap. (human: ~2 days / CC: ~30 min)

State: approved
Actual answer: A) owner-first stop barrier (recommended), user answer to D4
Accepted scope: Freeze stop/reload as: close RuntimeOwner admission under the stop barrier → provider Stopping → atomically revoke exact entry batch → drain existing owner permits → dispose exact scope. Never reopen admission after a failure; report Failed/residual for disposal failure.
History: none

### R4: canonical entry lease 的 identity 集合

Finding: 4, P0, confidence 9/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:60-64,84-88`。

Plan baseline: the plan requires provider identity, owner generation, mount identity and registry revision, but does not define the exact captured tuple or the final stale check boundary.

Runtime evidence: `src/capability/provider.rs:159-168,186-199` validates provider state/generation only. `src/capability/facade.rs:60-75` already models `BackendLease` with descriptor, owner generation and provider generation, while the producer still needs plugin/mount/entry identity to reject same-provider replacement.

Comparison grid:

| Contract | Current | A: full identity-fenced lease (recommended) | B: provider + registry lease |
|---|---|---|---|
| Captured identity | provider lease plus separate registry handle; owner/mount not uniformly bound | `(plugin_id, canonical_name, provider_generation, owner_generation, wasm_mount_identity, registry_revision/entry_handle, descriptor fingerprint)` | provider generation and registry revision/handle only |
| Replacement safety | incomplete for same provider generation or remounted owner | owner replacement, remount, descriptor change and registry replacement all become stale | same provider restart is fenced, but same-generation owner/mount replacement needs extra ad hoc checks |
| Post-guest validation | unspecified | validate the full tuple before accepting guest output | validate provider and entry only |
| Fallback | unspecified | any mismatch returns stale/fail-closed; never name-resolve | may need caller-specific owner/mount checks |

Question D5:
D5 — canonical entry lease 捕获哪些 identity？
Project/branch/task: `main` / Phase 2B entry fencing。
ELI10: lease 就像一张只允许进入某一栋楼、某一层、某一扇门的临时门卡。只检查 provider 还不够，因为同一个 provider 可能换了插件实例、WASM mount 或同名 entry。这个决定规定门卡上记录哪些身份，以及 guest 返回后是否再次核对。
Stakes if we pick wrong: an old call can return successfully after reload/replacement and its output can be accepted for the new plugin instance.
Recommendation: A because the full tuple makes same-provider replacement and remount stale at one shared validation boundary。
Completeness: A=10/10, B=6/10
Pros / cons:
A) full identity-fenced lease (recommended)
  ✅ Captures plugin, canonical name, provider generation, owner generation, mount identity, registry revision/handle and descriptor fingerprint, then rechecks all of them before accepting guest output.
  ❌ Expands the lease contract and requires every canonical call path to carry the same identity tuple through dispatch and completion.
B) provider + registry lease
  ✅ Reuses more of the existing ProviderHandle and RegistrationHandle APIs with a smaller immediate type change.
  ❌ Same-generation owner replacement or remount can pass provider and registry checks unless each caller grows its own extra fence, recreating inconsistent authority.
Net: choose one shared stale boundary instead of distributing owner/mount checks across callers。
Header: Lease identity
Options:
A) full identity-fenced lease (recommended)
Capture `(plugin_id, canonical_name, provider_generation, owner_generation, wasm_mount_identity, registry_revision/entry_handle, descriptor fingerprint)` and validate it before and after guest execution. Any mismatch returns stale and never falls back to name lookup. (human: ~2 days / CC: ~35 min)
B) provider + registry lease
Capture provider generation and the exact registry revision/handle, then let RuntimeOwner or callers add owner/mount checks where needed. Same-generation remount safety remains a caller obligation and must be separately tested. (human: ~1.5 days / CC: ~25 min)

State: approved
Actual answer: A) full identity-fenced lease (recommended), user answer to D5
Accepted scope: Freeze canonical lease identity as `(plugin_id, canonical_name, provider_generation, owner_generation, wasm_mount_identity, registry_revision/entry_handle, descriptor fingerprint)`. Validate the tuple before guest execution and again before accepting guest output; any mismatch returns stale/fail-closed and never falls back to name lookup.
History: none

### R5: observer 的 subscribe/snapshot 重连协议

Finding: 5, P1, confidence 9/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:92-96`。

Plan baseline: long-lived observers must recover from current snapshot after lag/reconnect, but the plan does not define the first-connect ordering or the snapshot/subscribe race window.

Runtime evidence: `src/capability/versioned_registry.rs:121-136` stores the snapshot before notification; `src/capability/zahir_facade.rs:406-435` already subscribes before taking the snapshot. `src/capability/versioned_registry.rs:615-635` confirms no replay for late subscribers. The observer contract must preserve that ordering and explicitly handle revision gaps.

Comparison grid:

| Contract | Current | A: subscribe-first snapshot anchor (recommended) | B: snapshot-first then subscribe |
|---|---|---|---|
| First connection | facade already uses subscribe then snapshot | subscribe, take snapshot/revision, discard `event.revision <= snapshot.revision`, apply later deltas | take snapshot then subscribe; requires a new atomic replay/lock contract |
| Lag/gap/close | notification only; recovery semantics partial | restart subscribe-first snapshot anchor | attempt to replay missing events or accept a race |
| Durable replay | forbidden | no replay; current snapshot is authority | would need new durable/replay authority |
| Implementation | existing pattern reusable | narrow observer helper and gap detector | new synchronization primitive or persistent queue |

Question D6:
D6 — long-lived observer 如何避免 snapshot/subscribe 窗口丢事件？
Project/branch/task: `main` / Phase 2B B5 observer contract。
ELI10: 观察者要同时“订阅未来变化”和“读取当前状态”。如果先读状态再订阅，中间发生的一次变化可能永远看不到。正确做法是先订阅，再读带 revision 的快照，并丢掉快照之前已经包含的通知。
Stakes if we pick wrong: a catalog or facade can silently keep a revoked entry, miss a replacement, or resurrect an old handler after reconnect.
Recommendation: A because it already matches `ZahirFacade` and gives a bounded recovery loop without inventing durable replay。
Completeness: A=9/10, B=5/10
Pros / cons:
A) subscribe-first snapshot anchor (recommended)
  ✅ Uses the existing subscribe-before-snapshot ordering, anchors the observer to a revision, and restarts the same procedure on lag, gap or channel close.
  ❌ Requires explicit gap detection and may rebuild the observer view more often during noisy registry churn.
B) snapshot-first then subscribe
  ✅ Looks simpler for a one-shot reader and may avoid a temporary subscription during snapshot construction.
  ❌ Leaves a race window unless a new atomic replay mechanism is introduced, which conflicts with the notification-only contract.
Net: reuse the proven snapshot-first recovery authority with subscribe-first ordering。
Header: Observer recovery
Options:
A) subscribe-first snapshot anchor (recommended)
Subscribe first, read the current snapshot/revision, discard notifications at or below the anchor, apply only later deltas, and restart from a fresh anchor after lag, revision gap or channel close. (human: ~1 day / CC: ~20 min)
B) snapshot-first then subscribe
Read the snapshot first and subscribe afterward. To preserve correctness, add an atomic replay/lock mechanism or document a tolerated race; no durable recovery source is added. (human: ~2 days / CC: ~25 min)

State: approved
Actual answer: A) subscribe-first snapshot anchor (recommended), user answer to D6
Accepted scope: Long-lived observers subscribe first, then read the current snapshot/revision anchor, discard events at or below the anchor, and apply later deltas. Lag, revision gap or channel close restarts this protocol; no durable replay or provider broadcast recovery is added.
History: none

### R6: compatibility route 迁移边界

Finding: 6, P1, confidence 8/10, reviewers `b0-architect` and `b0-plan-auditor`; plan `docs/research/aleph_modularization_phase2b_plan.md:84-88`。

Plan baseline: the compatibility inner route must keep rejecting calls without a real canonical identity and may not add name fallback, but the plan does not enumerate the old route's callers, define the migration point, or state when the old authority can be removed.

Runtime evidence: `src/executor/builtin_registry/registry/tool_registry_impl.rs:55-78` constructs plugin activation authority from metadata bindings; `src/extension/plugin_ops.rs:11-89` executes the compatibility path; `src/tools/registry.rs:759-773` still reports producer-not-mounted. The route census must distinguish this compatibility adapter from the canonical registry and forbid dual-write.

Comparison grid:

| Contract | Current | A: identity-preserving adapter with route census (recommended) | B: leave compatibility route until later |
|---|---|---|---|
| Authority | compatibility route can invoke owner path; canonical producer absent | all executable calls must resolve a canonical entry and pass existing gates; adapter only preserves identity | compatibility remains a separate executable path during B |
| Migration | unspecified | enumerate callers, switch one authority at B2/B4 boundary, remove old authority when all calls pass registry admission | defer census and route removal; keep explicit producer-not-mounted behavior for unmounted cases |
| Fallback | name fallback forbidden | no dual-write, no name lookup, no metadata-to-authority conversion | same prohibition, but two executable authorities remain |
| Proof | missing | route census + mixed/static regression + no-bypass test | only boundary rejection test; migration remains unresolved |

Question D7:
D7 — compatibility inner route 在 B 阶段如何迁移？
Project/branch/task: `main` / Phase 2B B4 canonical dispatch。
ELI10: 现在有一条旧通道可以调用插件 owner，另一条新通道会通过 canonical registry。问题是旧通道要不要在这次一起变成“只做身份适配”，以及何时删除它的独立执行权。
Stakes if we pick wrong: permission and generation checks can differ between two executable routes, so one route may bypass the canonical fencing even though the other is correct.
Recommendation: A because one callable authority prevents dual-write, name fallback and route-specific stale behavior while preserving the existing compatibility surface as an adapter。
Completeness: A=9/10, B=6/10
Pros / cons:
A) identity-preserving adapter with route census (recommended)
  ✅ Keeps one executable authority in `ToolHandlerRegistry`, preserves compatibility only as an identity-preserving adapter, and proves migration with a route census and no-bypass regression.
  ❌ Requires tracing every existing compatibility caller and may expose callers that need a separate follow-up before B can claim completion.
B) leave compatibility route until later
  ✅ Keeps B focused on the producer and avoids changing existing compatibility callers in the first implementation wave.
  ❌ Leaves two executable authorities with potentially different gate and stale semantics, so B cannot prove a single canonical dispatch boundary.
Net: choose whether B closes the dual-authority gap or explicitly accepts it as deferred scope。
Header: Compatibility route
Options:
A) identity-preserving adapter with route census (recommended)
Create a caller census, make the compatibility path an identity-preserving adapter into canonical registry admission, forbid dual-write/name fallback, and remove independent execution authority once all B callers use the registry. (human: ~2 days / CC: ~30 min)
B) leave compatibility route until later
Keep the compatibility execution route in place for this wave, preserving producer-not-mounted and no-name-fallback behavior, and record route unification as a later dependency. B completion must explicitly exclude single-authority proof. (human: ~1 day / CC: ~15 min)

State: approved
Actual answer: A) identity-preserving adapter with route census (recommended), user answer to D7
Accepted scope: Build a compatibility caller census, route executable compatibility calls through identity-preserving canonical registry admission, forbid dual-write and name fallback, and remove independent execution authority once all B callers use the registry. Keep producer-not-mounted for unmounted/no-identity cases.
History: none

## Review Sections / 四段评审

### 1. Architecture review

**Disposition:** Scope accepted as-is after D0/D1. R1-R6 are approved contracts and must be implemented together with their proof. No new architecture remedy remains pending.

```text
validated manifest/adapter + Ready RuntimeOwner + mount identity
                              │
                              ▼
                    producer preflight all entries
                              │
                              ├─ conflict/mismatch ──> no registry mutation
                              │                         Failed/residual diagnostic
                              ▼
             one coordinator commit: Ready + full registry snapshot
                              │
                              ▼
 ToolService gates → sealed PluginActivationAuthority → guest handler
                              │                                      │
                              └──── post-guest full lease validation ─┘

stop/reload:
close owner admission → provider Stopping → revoke exact batch
→ drain owner permits → dispose exact scope → Stopped or Failed/residual
```

Production failure coverage required by the architecture:

| Failure | Required behavior | Proof |
|---|---|---|
| registry commit conflict | old snapshot remains; owner is not publicly Ready | B2 barrier test, one revision/event assertion |
| second entry fails preflight | zero entries from the new plugin become visible | plugin batch transaction test |
| provider/owner/mount mismatch | resolve/invoke returns stale/fail-closed | tuple mutation matrix |
| stop races new call | admission closes before revoke/drain; no early dispose | real WASM barrier test |
| disposer failure | entry remains revoked; owner is Failed/residual; no false Disabled | EffectScope failure fixture |
| observer lag/close/gap | restart subscribe-first anchor from current snapshot | long-lived observer integration |
| compatibility caller bypass | no metadata/name authority; route enters canonical gates | route census + no-bypass regression |

Distribution architecture is unchanged: no new executable artifact, dependency, sidecar or release channel. The producer stays inside Rust Core and reuses existing WASM fixtures and registry/facade surfaces.

### 2. Code quality review

**[P1] (confidence: 9/10) `src/extension/canonical_handler.rs:15-68` —** the producer must replace the current test-only/dead-code handler boundary with a real identity-carrying binding, otherwise it will duplicate handler wrapping or bypass the registry. Reuse the existing `ProviderCallHandler`, `AdmissionHandler`, `ToolCallIdentity`, `RegistrationHandle` and `BackendLease`; do not add a second handler/disposer map.

**[P1] (confidence: 9/10) `src/tools/registry.rs:538-674` —** single-entry mutation APIs cannot express the approved plugin batch transaction by themselves. Add the smallest batch preflight/commit seam over the existing `VersionedRegistry` authority; do not expose a second registry or make callers compensate through ad hoc handles.

**[P2] (confidence: 8/10) `src/capability/provider.rs:159-199` —** provider generation alone is insufficient for same-provider owner replacement. The approved full lease tuple keeps owner generation, mount identity, registry revision/handle and descriptor fingerprint at one canonical boundary. Do not add caller-local stale checks with divergent semantics.

Shared-code decision: no new generic helper is approved. Existing first-party callers are `ScopedToolService` in `src/tools/scoped/dispatch.rs:250-347,1012-1052` and `ZahirFacade` in `src/capability/zahir_facade.rs:326-435`; their responsibilities differ, so the shared contract stays in the narrow registry/lease validation seam. No line-count savings are claimed; tests and integration are expected to add lines.

### 3. Test review

The test framework is Rust/Cargo, detected from `Cargo.toml`; no `CLAUDE.md` testing section was found. Existing tests cover many primitives, but the planned B3-B6 production chain is not yet proven.

```text
CODE PATHS                                      USER/CALLER FLOWS
[+] B1 manifest + Ready owner + mount          [+] model/user calls tool
  ├── identity tuple → staged entries             ├── [GAP][→E2E] all gates → guest → post-call fence
  └── [GAP] tuple field mutation matrix           ├── [GAP][→E2E] replacement during guest call
                                                   └── [GAP] stale error is recoverable
[+] B2 preflight → snapshot swap → revision     [+] publish plugin with 2 entries
  ├── [GAP][→E2E] second entry conflict            ├── [GAP] conflict leaves old snapshot
  └── [★★ TESTED] stale handle no-op               └── [GAP] exactly one revision/event
[+] B3 discovery/activation/stop                 [+] list/describe/resolve Cold plugin
  ├── [★★ TESTED] discovery no VM                 ├── [★★ TESTED] no read-side activation
  ├── [GAP][→E2E] activation then publish          └── [GAP] failed activation has no entry
  └── [GAP][→E2E] stop barrier + residual
[+] B5 observer snapshot recovery                [+] long-lived catalog/facade
  ├── [★★ TESTED] lag snapshot resync             ├── [GAP] publish→replace→revoke sequence
  └── [GAP] gap/close anchor restart              └── [GAP] builtin survives plugin revoke

COVERAGE: existing primitive coverage is strong, but B1-B6 production-chain coverage is incomplete.
QUALITY: existing ★★/★★★ tests cover primitives and selected real WASM paths; all listed GAPs are required proof for approved contracts.
```

Required Test Plan entries and value cards:

- **B1 unit/integration:** mutate each full lease field, including mount identity, descriptor fingerprint and entry revision. `Value: protects=full lease stale validation; fails_when=any field check is removed or replaced by name lookup; why_new=provider tests do not cover owner/mount/descriptor tuple; seam=none; no_claim=real guest acceptance remains an integration concern`.
- **B2 integration:** publish two entries with the second conflicting or invalid; assert old snapshot unchanged, no Ready-only public state, one revision and one event on success. `Value: protects=plugin all-or-nothing publication; fails_when=sequential partial registration or duplicate revision; why_new=single-entry registry tests cannot prove batch atomicity; seam=none; no_claim=does not prove WASM guest execution`.
- **B3 real-WASM integration:** successful activation publishes only after mount/binding proof; failed producer leaves zero canonical entries; repeated list/describe/resolve leaves VM/provider/registry generation unchanged. `Value: protects=Ready-only producer boundary; fails_when=read-side activation or ghost entry; why_new=existing discovery and lifecycle tests are not connected to producer; seam=none; no_claim=external process distribution`.
- **B4 real-WASM E2E:** call through `ScopedToolService` gates into a guest handler, replace owner/mount while guest is blocked, and assert post-guest stale rejection. `Value: protects=sealed dispatch and post-guest fence; fails_when=output accepted after replacement; why_new=canonical handler has only structural unit coverage; seam=none; no_claim=provider network acceptance`.
- **B5 integration:** one existing observer sees publish→replace→revoke, then lag/close/gap recovery from a fresh snapshot without resurrecting old entry and without removing builtin entries. `Value: protects=long-lived projection correctness; fails_when=event window or stale replay restores old state; why_new=existing lag test does not cover the full lifecycle sequence; seam=none; no_claim=durable replay, which remains out of scope`.
- **B6 real-WASM/barrier integration:** block a guest call, run stop, prove drain before dispose; inject disposer failure/provider mismatch/late completion and assert residual plus replacement availability. `Value: protects=stop and replacement failure semantics; fails_when=early unload, false Disabled or old completion deletes replacement; why_new=existing tests cover selected barriers but not the full failure matrix; seam=none; no_claim=performance throughput`.

Tests made obsolete: none identified. Existing primitive and fixture tests remain valuable and should be extended rather than removed.

### 4. Performance review

No benchmark or production rate was available, so no speedup or memory claim is made. The plan must measure, rather than assume, these risks:

- **[P1] (confidence: 8/10) `src/extension/runtime_owner.rs:45-65` and `src/extension/lifecycle.rs:1218-1233` —** the current 1024-permit drain has no timeout, so a blocked guest can hold stop indefinitely. The approved owner-first order fixes the race but not the indefinite wait. B6 must measure stop p50/p95/max for 0, 1, 1024 in-flight calls and one intentionally blocked call, and must define the residual/error result at the drain deadline.
- **[P1] (confidence: 8/10) `src/capability/versioned_registry.rs:96-126` —** atomic snapshot publication serializes writers and may clone or rebuild a large snapshot. Measure publish/revoke latency and allocations for 1, 2, 10 and the largest real plugin export set; no throughput claim is allowed before measurement.
- **[P2] (confidence: 7/10) `src/capability/zahir_facade.rs:406-435` —** lag/gap recovery rebuilds from the current snapshot and may allocate under churn. Measure rebuild latency and allocation at the largest observed snapshot, recording unknown scale instead of inventing a threshold.

Performance acceptance is deferred to B6 evidence and does not change the functional contract: snapshot publication remains atomic, stop remains owner-first, and observer recovery remains snapshot-based.

Additional performance budget receipts from the review: `src/tools/registry.rs:553-590` and `src/capability/versioned_registry.rs:96-126` serialize writers and rebuild/clone records and projections in the write critical section; measure lock wait/hold, allocation bytes and O(N+K) batch cost. `src/tools/registry.rs:778-810` and `src/capability/zahir_facade.rs:158-190` rebuild maps while old `Arc` snapshots may retain prior handler generations; measure RSS and live generations during replace churn. `src/capability/provider.rs:201-283` and `src/capability/zahir_facade.rs:340-400` add mutex and repeated lease checks on the hot path; measure p95 lock wait and QPS at concurrency 1/32/256. `src/capability/versioned_registry.rs:47-70` and `src/capability/zahir_facade.rs:278-320,406-419` use bounded rings (256 registry, 16 provider), so lag is bounded but can trigger O(N) rebuild; measure lag frequency and rebuild RSS. `src/extension/effects/scope.rs:87-103` and `src/extension/loader.rs:505-540` serialize disposal and have no local timeout; B6 must measure each step and preserve residual state on deadline. Plugin-instance linear-memory/RSS/entry limits are unknown and must be measured before claiming a runtime budget.

## NOT in scope

- Cold activation proxy: deferred because Ready-only is an approved B0 boundary; activation proxy needs a separate permission/visibility/confirmation/lease decision.
- MCP lazy, sidecar/dylib, scheduler, Safe Replay, EffectClaim producer, cross-kind capability expansion: explicitly prohibited by the plan.
- New durable replay source: deferred; provider/registry broadcast remains live notification only and SessionEventStore remains the durable recovery authority.


## Outside Voice / 独立挑战

Coverage: native fallback used because the Codex CLI was not installed (`CODEX_BIN=not_installed`); one bounded read-only Plan reviewer completed. This is missing external-provider coverage, not a clean external approval.

```text
发现以下未被当前 B0 记录充分解决的问题：

- Critical，置信度 9/10 — docs/research/aleph_modularization_phase2b_plan.md:226-231 与 src/capability/provider.rs:260-283、src/capability/versioned_registry.rs:100-145
  R1 选择“Ready 与 registry snapshot 单一提交点”，但现有 provider 和 registry 各自拥有独立 mutex、revision 与广播；没有跨对象事务或共同线性化锁。仅增加 coordinator 仍无法让两个已有读者原子观察，除非所有 snapshot()、transition() 和 registry mutation 都改为经过同一个边界。
  建议：先定义统一的 CanonicalPublication 状态容器和唯一读 API，把 provider 状态、owner identity、entry snapshot 放入同一版本；如果不改变读取面，则撤回原子 Ready 方案，采用明确的内部 Starting/Prepared 状态并证明其永不 executable。

- High，置信度 10/10 — docs/research/aleph_modularization_phase2b_plan.md:72-80 与 src/extension/lifecycle.rs:1218-1233
  计划批准的 stop 顺序是先关闭 admission，再进入 Stopping、撤回 entry、drain；实际代码先 acquire_many_owned(1024)，随后才 close_admission()。因此 drain 等待期间仍允许新调用进入，持续负载下可能永远无法拿满 permit。
  建议：B3 前把 close_admission() 作为 stop barrier 第一动作，并在同一 barrier 下完成 provider transition 与 entry revoke；以有界 drain deadline 将未完成调用转为 residual。

- High，置信度 9/10 — docs/research/aleph_modularization_phase2b_plan.md:92-96 与 src/tools/registry.rs:792-818、src/capability/provider.rs:260-283、src/capability/zahir_facade.rs:406-435
  long-lived observer 只订阅 registry publication，但 provider Ready/Stopping/Failed 变化不会产生 registry event 或 revision；仅 subscribe-first 不能修复缺失事件。
  建议：把 provider lifecycle transition 纳入同一 canonical publication，或让 observer 同时订阅 provider changes 并以统一 canonical revision 重建。

- High，置信度 9/10 — docs/research/aleph_modularization_phase2b_plan.md:60-64,84-88 与 src/tools/registry.rs:684-692、src/extension/runtime_owner.rs:207-227
  R4 定义完整 lease tuple，却没有定义如何原子捕获它。resolve_entry()、provider snapshot 和 owner mount identity 来自独立读取，replacement/remount 可在读取之间发生。
  建议：增加 producer-owned immutable ReadySnapshot，在 owner 控制锁下同时捕获 owner generation、mount identity、binding 和 registry revision；lease 只能从该 snapshot 派生。

- Medium，置信度 8/10 — docs/research/aleph_modularization_phase2b_plan.md:68-72,84-88 与 src/tools/registry.rs:684-692、src/tools/registry.rs:1291-1305
  当前 registry 允许 provider 未 Ready 时注册 callable record，之后 provider 独立转 Ready 即自动出现在 snapshot_state() 和 resolve_entry()；普通 register() 仍可能绕过 producer contract。
  建议：plugin entry 先标 staged/unpublished，只有 canonical coordinator 提交后进入 callable projection；普通 register() 继续服务既有 builtin/MCP 路径，并加 source contract test。

Recommendation: 先冻结统一 canonical publication/read model 并修正 stop barrier，再开始 B1，因为当前批准的原子发布、observer 可见性和 lease 身份证明在现有分离状态源上无法成立。
```

### R7: canonical publication/read model

Finding: Outside Voice Critical, confidence 9/10; independent Plan reviewer; exact receipts are quoted above.

Plan baseline: R1 approved D2 chose a coordinator that stages identity/entries and makes Ready plus canonical snapshot public in one commit. R4 approved a complete provider/owner/mount/entry tuple. R5 approved snapshot-first observer recovery.

Runtime evidence: `src/capability/provider.rs:260-283`, `src/capability/versioned_registry.rs:100-145`, `src/tools/registry.rs:792-818`, and `src/capability/zahir_facade.rs:406-435` expose separate state/revision/notification paths. `src/tools/registry.rs:684-692` and `src/extension/runtime_owner.rs:207-227` do not provide one atomic lease-capture boundary. Exact atomicity is therefore unverified and the reviewer has identified a concrete contradiction.

Comparison grid:

| Contract | Current / approved baseline | A: unified canonical publication/read model (recommended) | B: staged internal state, withdraw cross-object atomicity |
|---|---|---|---|
| publication authority | coordinator order is approved but underlying state sources remain separate | one immutable `CanonicalPublication` version contains provider state, owner/mount identity and callable entry snapshot; one read API | provider/registry remain separate; public contract becomes fail-closed eventual consistency, not one atomic Ready+registry commit |
| lease capture | tuple fields are approved but captured from separate reads | lease derives only from producer-owned immutable ReadySnapshot | lease must tolerate no atomic cross-source capture; stale checks remain fail-closed but do not prove a simultaneously existing tuple |
| observer | registry subscribe-first plus snapshot | observer subscribes to canonical publication and rebuilds by canonical revision | observer subscribes to multiple sources and may observe intermediate internal states; no executable visibility until explicit staged publish |
| stop / transition | owner-first close admission is approved | lifecycle transition emits a new canonical publication version and closes old lease | lifecycle changes remain separate; caller must accept bounded lag and explicit non-executable intermediate states |
| implementation | extend original registry/facade/runtime files, preserve existing gates | add the smallest canonical container/read API and migrate all executable readers; larger B1/B2 surface | retain current surfaces, add staged/unpublished marker and remove the atomicity claim; requires rewriting B0/B1/B2 acceptance language |
| proof | B1/B2/B5 tuple, publish and observer tests | prove no reader sees mixed versions, one canonical revision/event per transition, and old lease rejection | prove staged state is never executable, no reader claims atomicity, and eventual resync is fail-closed |

D8 — publication boundary
Project/branch/task: `main` / Aleph Phase 2B B1-B2.
ELI10: 当前系统把“插件准备好了”和“工具清单已经发布”放在几个不同的状态盒子里。只按顺序更新这些盒子，读者仍可能在中间看到互相不匹配的版本。问题是要把它们放进一个共同的只读快照，还是降低承诺、明确允许中间状态但禁止执行。
Stakes if we pick wrong: a mixed provider/owner/entry tuple can accept or reject the wrong call, while an observer can publish a catalog that never existed as one valid runtime state.
Recommendation: A because it preserves the already-approved atomic Ready-only and canonical lease contracts and makes them provable through one authority.
Completeness: A=10/10, B=6/10
Pros / cons:
A) unified canonical publication/read model (recommended)
  ✅ Gives provider state, owner identity, mount binding, entry snapshot and canonical revision one immutable read boundary, so dispatch and observers can prove they saw one valid runtime generation.
  ✅ Preserves the existing Ready-only, fail-closed, snapshot recovery and replacement contracts without relying on call ordering between independent mutexes.
  ❌ Requires migrating every executable reader and adds a larger B1/B2 integration surface before producer work can begin.
B) staged internal state, withdraw cross-object atomicity
  ✅ Keeps the existing state containers and limits the first implementation wave to staged markers, explicit non-executable states and fail-closed resync behavior.
  ❌ Weakens the approved atomic Ready+registry and lease-capture guarantees, so the plan and its acceptance tests must be rewritten before implementation.
  ❌ Leaves more than one state source for observers and makes mixed-version reasoning a permanent caller burden.
Net: choose whether the plan keeps a provable single publication authority or explicitly trades that guarantee for a weaker staged contract.

Header: Publication boundary
Options:
A) unified canonical model (recommended)
Create the smallest producer-owned immutable canonical publication/read model, migrate executable readers and observers to it, and preserve the approved atomic Ready-only/lease/recovery contracts. (human: ~3 days / CC: ~45 min)
B) staged weaker contract
Keep provider, owner and registry state sources separate, add explicit non-executable staged state, and rewrite B0/B1/B2 acceptance from atomic publication to fail-closed eventual consistency before implementation. (human: ~2 days / CC: ~30 min)

State: approved
Actual answer: A) unified canonical model (recommended), user answer to D8
Accepted scope: Add the smallest producer-owned immutable CanonicalPublication/ReadySnapshot and one canonical read API. Put provider state, owner/mount identity, binding, entry snapshot and canonical revision in the same immutable version; migrate executable readers and observers to that read boundary. Preserve Ready-only, full lease fencing, snapshot recovery, identity-preserving compatibility adapter, owner-first stop and residual failure semantics. Add tests proving no mixed-version reads, one canonical revision/event per transition, stale lease rejection, observer recovery, and staged entries never executable.
History: none

## Reality rows / 现实核验

| Row | Verdict | Receipt | Evidence |
|---|---|---|---|
| premise-table | pass | `docs/research/aleph_modularization_phase2b_plan.md:463-589` | Architecture, tests, performance, exclusions and outside-voice claims each carry source receipts or are explicitly marked as measurements/unknowns. `unverified=0` for the required review claims; future runtime measurements remain `[re-measure before gate]`. |
| already-done | pass | `git log origin/main --since=2026-10-20 -- docs/research/aleph_modularization_phase2b_plan.md src/tools/registry.rs src/capability/provider.rs src/capability/versioned_registry.rs src/capability/zahir_facade.rs src/extension/runtime_owner.rs src/extension/lifecycle.rs` | No already-merged Phase 2B producer implementation was found; history shows prerequisite lifecycle/provider commits only. |
| surface-check | pass | `gstack-plan-reality --help`, `python qa/browser_live/check_memory.py --self-test`, `python qa/browser_live/check_memory.py` | Required command surfaces returned rc 0; memory self-test passed and probe returned `available_bytes=6925533184 required_bytes=4294967296 ALLOW`. Cargo was not run during review. |
| binding-decisions | pass | `docs/research/aleph_modularization_phase2b_plan.md:190-640` and `gstack-decision-search --scope branch --recent 50` | D1-D8 are recorded in the plan; branch decision search returned no conflicting decision. Approved choices remain fixed in the accepted scope. |
| numbers | pass | `docs/research/aleph_modularization_phase2b_plan.md:545-553`, shell receipts 642 plan lines / 5376 test-plan bytes / 1 changed file | Existing numbers are code/config values or direct counts. Human/CC effort estimates are labeled estimates; performance thresholds are explicitly deferred to measurement. |
| deferred-asks | pass | `docs/research/aleph_modularization_phase2b_plan.md:555-560` | Cold proxy, MCP lazy, sidecar, scheduler, Safe Replay, EffectClaim producer, cross-kind expansion, durable replay, harness changes and repository mutations are explicitly deferred/prohibited. No linked issue was supplied. |
| pr-count | pass | `.gstack/ship-policy.json` absent; default one PR per wave | Two-wave delivery is approved; default is one PR per wave, with B1/B2 first and B3-B6 second. No extra PR split is proposed. |

REALITY: premise-table pass all factual review claims have source receipts or explicit unknown/measurement labels `docs/research/aleph_modularization_phase2b_plan.md:463-589`
REALITY: already-done pass no merged producer implementation found in origin history `f1a1b09d5` (git log origin/main --since=2026-10-20 for the named plan/source paths)
REALITY: surface-check pass plan-reality help, memory self-test and memory probe returned rc 0; probe allowed 6925533184 bytes `qa/browser_live/check_memory.py:5-12` and `docs/research/aleph_modularization_phase2b_plan.md:117-124`
REALITY: binding-decisions pass D1-D8 recorded and no conflicting branch decision returned `docs/research/aleph_modularization_phase2b_plan.md:190-640`
REALITY: numbers pass counts and estimates are labeled; runtime thresholds remain measurement gates `docs/research/aleph_modularization_phase2b_plan.md:545-553`
REALITY: deferred-asks pass all omitted scope is named with reason and no linked issue was supplied `docs/research/aleph_modularization_phase2b_plan.md:555-560`
REALITY: pr-count pass default one PR per wave and approved two-wave delivery `docs/research/aleph_modularization_phase2b_plan.md:640`

## GSTACK REVIEW REPORT

| ID | Severity | Confidence | Finding | Evidence | Disposition |
|---|---|---:|---|---|---|
| R1 | P0 | 9/10 | Ready and registry publication originally lacked a provable shared linearization boundary. | `src/capability/provider.rs:260-283`; `src/capability/versioned_registry.rs:100-145`; plan R1/D2 | Approved D2, strengthened by D8: immutable canonical publication/read model is required. |
| R2 | P0 | 9/10 | Multi-entry plugin publication could partially expose entries without a batch transaction. | `src/tools/registry.rs:538-674`; plan R2/D3 | Approved D3: preflight plus one snapshot swap/revision/event. |
| R3 | P1 | 10/10 | Existing stop path drains permits before closing admission and has no deadline. | `src/extension/lifecycle.rs:1218-1233`; plan R3/D4 | Approved D4: close admission first, then transition/revoke/drain; B6 measures bounded residual behavior. |
| R4 | P0 | 9/10 | Full lease tuple could be assembled from independent reads and never have existed together. | `src/tools/registry.rs:684-692`; `src/extension/runtime_owner.rs:207-227`; plan R4/D5 | Approved D5 plus D8: derive lease only from immutable ReadySnapshot. |
| R5 | P1 | 9/10 | Registry-only observer events cannot represent provider lifecycle changes. | `src/capability/provider.rs:260-283`; `src/capability/zahir_facade.rs:406-435`; plan R5/D6 | Approved D6 plus D8: canonical lifecycle publication/revision feeds observers. |
| R6 | P1 | 9/10 | Compatibility execution could retain independent name/handler authority. | `src/tools/scoped/dispatch.rs:250-347,1012-1052`; plan R6/D7 | Approved D7: identity-preserving adapter, route census, no dual-write/name fallback. |
| R7 | P0 | 9/10 | Coordinator call ordering alone cannot make separated provider/owner/registry readers atomic. | Outside Voice, exact output at plan `:562-589`; plan R7/D8 | Approved D8: unified immutable CanonicalPublication/ReadySnapshot and one canonical read API. |
| PERF-1 | P1 | 8/10 | Stop, publication, snapshot retention, observer rebuild, lock contention and disposal budgets are unmeasured. | `src/extension/runtime_owner.rs:45-65`; `src/capability/versioned_registry.rs:96-126`; plan Performance `:545-553` | Accepted as B6 measurement obligations; no unverified performance claim made. |

VERDICT: PASS — B0 plan review complete and ready for delegated B1/B2 implementation after the approved D8 architecture amendment. Outside voice used the required native fallback because Codex CLI was unavailable; this is recorded as `outside_status: native-fallback-completed`, not external-provider clean approval. The review plan is uncommitted and contains review artifacts only; production implementation has not started.

UNRESOLVED_DECISIONS: none
UNRUN_VALIDATION: no cargo check/test/clippy/format commands were run during review; the plan's memory-gated cargo protocol remains mandatory for implementation and validation.
