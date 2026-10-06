# Capability Phase 4 后续 Gate（E→F→G）— 架构书面 spec

| 项目 | 取值 |
| --- | --- |
| 文档性质 | 架构书面 spec（非已实现代码；不修改产品代码，不提交） |
| 适用分支 | `capability-phase4-follow-up` |
| 范围基线 | 已批准「实现 Gate E→F→G，Gate H/I 延后」 |
| 文档基线 | `1ea05b1ce86d35b64384d8157f015fd68224cdf6`（`main` Phase 4 基座） |
| 上游 spec | `docs/superpowers/specs/2026-10-06-capability-phase4-design.md` |
| 上游 prompt | `docs/superpowers/prompts/2026-10-06-capability-phase4-follow-up-gates.md` |
| 日期 | 2026-10-06 |

> 本文档只描述设计意图，不修改任何源码。文中所有 `src/...` 路径用于定位既有符号；所有「应当 / 将」均为设计约定。凡现有源码事实与本文档冲突，以源码为准，并应在 implementation plan 阶段由现有源码重新核验装配点（见 §9）。

---

## 1. 目标、成功标准与非目标

### 1.1 目标

在 Phase 4 基座（Gate A–D 已合并）之上，把「纯 contract + 纯 value 状态机」接到真实运行时，分三步落地：

- **Gate E**：concrete `Zahir` facade/backend wiring —— 把 `ToolBackendAdapter` 升级为唯一 Tool facade 的 live backend，但不建立第二套 registry，不复制 descriptor/handler 事实。
- **Gate F**：EffectClaim producer 与 recovery integration —— 在 design review 后决定是否实现 producer；先钉死 claim / budget reservation / owner generation / fencing token 的事务边界与每个 crash 窗口，不实现 automatic replay invocation。
- **Gate G**：committed projection/subscription wiring —— 把纯 `CapabilityChangeStream` 接到真实 committed registry change feed，并证明 ACP/MCP/model projection 不能授权、不能 resolve handler、不能决定 replay、不能维护第二份 identity/revision。

### 1.2 成功标准（验收判据）

1. **同代一致性**：snapshot / handler / descriptor 三者在同一 atomic registry snapshot 上派生（`describe` 与 dispatch 用同一代）。禁止 describe 走 `descriptor_snapshot()`、dispatch 走 `resolve()` 导致代际撕裂。
2. **stale lease fail-closed**：`resolve` 返回的 `BackendLease` 在其 owner generation 落后于当前 `generation()` 时被判定失效，且失效后不得取得 handler。
3. **无第二套 registry**：E/F/G 全程不新增 handler map / capability map；`ToolHandlerRegistry` 仍是 callable Tool 唯一事实源；`OwnershipTree` 是唯一 ownership 事实源（不旁路它自造 owner 计数）。
4. **projection 与 dispatch 同代**：projection 只读 canonical snapshot，不旁路 registry。
5. **change cursor 恢复且无重复**：断线重连后 `CapabilityChangeStream` 从已提交 cursor 恢复，`(CapabilityId, CapabilityRevision)` 去重，永不从零开始。
6. **fail-closed 全覆盖**：Unknown / missing identity / stale owner / schema mismatch / invalid claim 一律 fail-closed（§5）。
7. **不变量不破坏**：上游 prompt 列出的 10 条当前不变量全部保持（见 §1.4 引用）。

### 1.3 非目标（明确排除，本期不实现）

| 非目标 | 归属 gate | 说明 |
| --- | --- | --- |
| 完整 ACP server gate | H（延后） | 不审计 inbound command→Session/Run mutation line、不实现 ACP transport/session manager 全链路；ACP JSON 已有 projection 不构成「ACP server 完成」的证据 |
| automatic Safe Replay / external-effect exactly-once | I（延后） | 不实现真实 replay invocation；`ReplayPermit` 保持 `pub(crate)`、single-use、`effective_input: None` 生产侧无条件拒绝 |
| universal durable scheduler | I（延后） | 不引入任何 scheduler |
| 其余 8 类 kind backend | 未定 gate | Skill/Agent/Task/Resource/EventSource/Subscription/Plugin/Hook 仍只有 contract/deferred，不挂载 |
| `CapabilitySlot` 动态化 | 禁止 | 保持启动期 first-writer-wins |
| `src/harness/` 扩张 | 禁止（R10） | 不新增 harness 业务文件承载 facade 业务 |
| 通过 `to_metadata_form` 生成 canonical identity | 禁止 | 仍只做旧 Tool metadata compatibility wrapper |

### 1.4 当前不变量（来自上游 prompt，本期不得破坏）

1. `ToolHandlerRegistry` 是 callable Tool 唯一事实源。
2. Descriptor 与 handler 必须同代配对。
3. `CapabilityRevision` ≠ `OwnerGeneration`（`src/tools/registry.rs` 的 revision 是工具注册变更计数；`OwnerGeneration` 是 owner 发放计数，二者禁止别名）。
4. `VisibilityScope` ≠ `LifetimeScope`（正交轴）。
5. Projection 不授权，notification 不持久化。
6. `SessionEventStore`（`src/session/store.rs`）是 recovery-related committed source；`StateDatabase` / ACP JSON 是 projection-only。
7. Unknown、missing identity、stale owner、schema mismatch、invalid claim 都 fail-closed。
8. `to_metadata_form` 只做旧兼容 wrapper（`src/tools/service.rs:453`）。
9. `src/harness/` 不扩张为第二个 runtime。
10. automatic Safe Replay、external exactly-once、universal scheduler 仍是未批准 gate。

---

## 2. 参考项目 gap analysis

### 2.1 可映射 vs 不可复制的模式

| 参考机制 | 可映射到 Aleph 的部分 | 不可复制的部分（原因） |
| --- | --- | --- |
| **LangGraph checkpoint**（`docs/reference/GRAPH_LAYER.md`） | checkpoint 概念映射到 Aleph 的 `SessionEventStore`（append-only log + 单调 seq + cursor）；「从 checkpoint 恢复并重放」映射到 `load_events_range` + `CapabilityChangeStream` cursor resync | LangGraph 的 checkpoint 是「图状态快照 + 每步保存」；Aleph 的 recovery 事实源是事件日志而非状态快照，且 StateDatabase 是 projection 不能当 checkpoint 来源。禁止照搬「把 projection 当恢复事实源」的读法 |
| **ACPX / ACP**（`src/acp/*`、`docs/superpowers/specs/2026-04-18-acp-harness-refactor.md`） | ACP 的 session/run ownership、committed event projection、cursor/断线恢复，是 Gate G 的 projection 边界参考 | ACP 的 **inbound command → Session/Run mutation line** 属于 Gate H，本期不实现。ACP server 若复用 Zahir projection 与 existing session/run ownership，必须等 H 单独审计；本期只把 ACP JSON 当作 projection 面，不把它当作 capability registry 或授权面 |
| **deepseek-harness**（Cordis 式 `static inject` / 插件树） | `CapabilitySlot::install/decline` 的「正确的事是唯一可构造的事」（`MetaGuard` 惯用法）借鉴了其「waiting for: sessionPersistence」的 unsatisfied-inject 形状（见 `src/capability/mod.rs` 模块文档） | **仓库里并不存在 `deepseek-harness/packages/capability/` 这个路径**。经核验：`/Volumes/TBU4/Workspace/deepseek-harness` 与 `/Volumes/TBU4/Workspace/deepseek-harness/packages/capability/` 均不存在（`ls` 返回 No such file or directory）。任何「deepseek-harness 有 capability 包可参考」的说法都不成立——它是一个被引用的、但实际不存在的装配点，后续 implementation plan 不得把它写成事实源或复用目标 |

### 2.2 结论

- 可映射的只有**语义**（checkpoint→event-log cursor、ACP projection→只读投影、Cordis inject→slot 惯用法），没有可复制的**代码**。
- deepseek-harness 的 capability 目录是**空引用**，必须从 gap analysis 中剔除，不能作为 Gate E–G 的实现参考。

---

## 3. Aleph 架构映射（E / F / G）

### 3.1 三类边界（authority / projection / notification）

| 边界 | 事实源 | 行为约束 |
| --- | --- | --- |
| **authority**（权威） | `ToolHandlerRegistry`（callable handler 唯一事实源）；`OwnershipTree`（owner generation / revoke / dispose 唯一事实源）；`SessionEventStore`（recovery 唯一 committed source） | 只有 authority 能决定「能否调用」「lease 是否 stale」「能否 replay」。任何 projection/notification 不得承担该职责 |
| **projection**（投影） | `StateDatabase`（`src/resilience/database/state_database/mod.rs`）、ACP JSON（`src/acp/manager/persistence.rs`）、MCP 类型/工具视图（`src/mcp/types.rs`、`src/tools/mcp_scope_view.rs`、`src/tools/server/ops.rs`） | 只读、只供展示/查询，不授权、不 resolve handler、不决定 replay、不维护第二份 identity/revision。禁止从 projection 反推 lease 或 owner generation |
| **notification**（通知） | `GlobalBus`（`src/event/global_bus.rs`） | 只做低延迟通知，无 cursor、无 generation、不持久化。禁止把 GlobalBus 消息当作 change feed 或 recovery 依据 |

**装配点（需 implementation plan 阶段由源码确认）**：`Zahir` trait 目前无 concrete impl；`ToolBackendAdapter` 只实现 `CapabilityBackend`（`lookup/enumerate/generation`），未实现 `Zahir`。concrete facade 是把 `CapabilityBackend` 组合进 `Zahir` 的新类型（下文记 `ConcreteZahir`），其具体落点与名字待 plan 阶段定，此处不发明新类型名。

### 3.2 Gate E — concrete facade/backend wiring

**目标**：`ConcreteZahir` 持有 Tool backend 而不复制 descriptor/handler truth。

设计约定（逐条对应上游 Gate E 的「必须回答并实现」）：

1. **facade 持有 backend 的方式**：`ConcreteZahir` 持有 `ToolBackendAdapter`（内含 `ToolHandlerRegistry`），外加 `OwnershipTree` 的 `Arc`（用于 `resolve` 时读真实 owner generation）。不持有第二份 descriptor/handler map——所有 descriptor 与 handler 都从 `ToolHandlerRegistry` 现有 snapshot 派生。
2. **`describe(scope)`**：从 `self.registry.descriptor_snapshot()`（`src/tools/registry.rs:487`）这个 atomic `Arc<HashMap>` 生成 `CapabilitySnapshot`，`committed_cursor` 用 registry 的 `revision()`（`src/tools/registry.rs:501`）。禁止分别多次读 registry 拼装。
3. **`resolve(reference)`**：经 `CapabilityBackend::lookup`（返回 `Option<CapabilityDescriptor>`）取 descriptor，组合进 `BackendLease`（`src/capability/facade.rs` 定义，字段为 `descriptor + owner_generation`），`OwnerGeneration` 来自 `OwnershipTree`（而非 `ToolBackendAdapter::generation()` 的常量 0）。stale revision / stale owner generation 时 fail-closed（返回无 lease 或明确 stale 标记）。**注意**：`lookup` 只返回 descriptor，`BackendLease` 是 `resolve` 层组装的结果；本期需补一个「stale 判定」的装配点——由 plan 阶段确认放在 facade 还是 backend 边界。
4. **`subscribe(scope, cursor)`**：从 `ToolHandlerRegistry::subscribe()`（`src/tools/registry.rs:541`，返回 `broadcast::Receiver<RegistryChange>`）接入 `RegistryChange`，映射到 `CapabilityChange`（`Registered/Replaced/Unregistered` 一一对应，`revision` 取自 `RegistryChange.revision`），并驱动 `CapabilityChangeStream`。首次订阅先发 snapshot + committed cursor。
5. **projection 只读 canonical snapshot**：`project()` 复用 `describe()` 的 snapshot，不旁路 registry。
6. **schema fingerprint 计算**：`CapabilityDescriptor.schema.fingerprint`（`src/capability/descriptor.rs` 的 `SchemaRef`）当前为 `[0u8; 32]`（`src/capability/backend.rs::to_descriptor` 明确未计算）。E 必须回答：由 describe/projection 路径负责计算，canonical schema serialization 由单一 owner 负责（不能 descriptor 与 projection 各算一份导致判据 #1「同一事实的两份表述」）。
7. **`OwnerGeneration(0)` 接管**：`ToolBackendAdapter::generation()`（`src/capability/backend.rs:73`）当前返回常量 `OwnerGeneration(0)`，其注释明确「Task 2（OwnershipTree）替换此常量」。E 的 `resolve` 必须读 `OwnershipTree` 的真实 generation，不能再依赖该常量。

**禁止**（同上游 Gate E）：`dyn Any`、巨大 payload enum、第二套 handler map、把 `CapabilityRevision` 与 `OwnerGeneration` 合并、`CapabilitySlot` 动态化、改 `src/harness/`、经 `to_metadata_form` 生成 canonical identity。

**已知 gap（E 必须关闭）**：
- `SchemaRef.fingerprint` 未计算（`backend.rs::to_descriptor`）。
- `ToolBackendAdapter::generation()` 恒 `OwnerGeneration(0)`。
- `Zahir` 无 concrete impl，`describe/resolve/subscribe/project` 四方法无实现。
- `BackendLease` 目前只带 `descriptor + owner_generation`，无 stale 判定路径。

### 3.3 Gate F — EffectClaim producer 与 recovery integration

**目标**：先做 design review，再决定是否实现 producer。本 spec 只钉死 producer 的语义边界，不承诺实现 automatic replay。

**现有 reducer/adapter 事实**（`src/capability/effect_claim.rs`）：
- `effect_claim_event_from_session(&SessionEvent) -> Option<EffectClaimEvent>`：把 4 个 claim 事件（`Prepared/Claimed/Invoking/Terminal`）转成 reducer 输入；非 claim 事件被忽略。
- `reconcile_effect_claim(&[EffectClaimEvent]) -> EffectClaimReconciliation`：纯函数，合法状态迁移闭包为 Prepared→Claimed→Invoking→(Succeeded|Failed)；dispose/revoke/unobservable 从任意 active（非 terminal）状态可落 `Unknown`，且 `Unknown` 是 terminal。malformed identity / duplicate active claim / 非法迁移 / terminal 后再迁移 → `Unknown`（fail-closed）。

**producer 设计约定**（若 plan 阶段批准实现）：

1. **真实生产路径**：哪些真实路径会产生 `EffectClaimPrepared/Claimed/Invoking/Terminal` 事件，须在 plan 阶段枚举——候选是 scoped tool dispatch（`src/tools/scoped/dispatch.rs`）与 MCP 面（`src/gateway/mcp_face/mod.rs`，已补 per-call `CallIdentity`）。**本 spec 不预先断言具体路径**，标注为待源码确认装配点。
2. **事务边界**：claim、budget reservation、owner generation、fencing token 必须在**同一次** `SessionService::emit_batch` 里提交（原子提交；`InProcessActorSessionService` 是唯一生产 override，`SessionEventStore::append_batch` 单事务落盘）。禁止把 claim 与 budget 拆成两次 batch。
3. **crash 窗口**（每个窗口都要在 design review 枚举，不能靠文档措辞掩盖）：
   - crash after intent（`Prepared` 前）→ 无 claim 记录，下次重来；
   - crash after claim（`Claimed` 后、`Invoking` 前）→ reducer 闭包为 Claimed，非 terminal；
   - crash after invocation（`Invoking` 后、`Terminal` 前）→ 外部 effect 可能已发生，reducer 无 terminal → `Unknown`；
   - crash after outcome（`Terminal` 后）→ terminal 已 durable，重放不得再次调用。
4. **VerifyOnly 路径**：`ReplayPermit::prepare` 在 `effective_input: None`（生产侧无条件）时返回 `ReplayPrepare::Refused`，落到 VerifyOnly/fail-closed；`REPLAY_MAX_ATTEMPTS_PER_CALL = 3` 是跨崩溃 cursor 预算上限，耗尽 ⇒ VerifyOnly（`src/session/replay.rs`）。这些保持不变。
5. **职责分离**：`ReplayPermit`（bridge 侧 single-use invoke）与 `EffectClaim` reducer（纯 reconciliation）保持分离；reducer 不 invoke，permit 不 reconcile。
6. **外部 effect 不可观测 → Unknown**：外部 effect 无 ledger 时，`Terminal` 缺失 → `Unknown`，且不得推断成功（对应 §7.5「Unknown 是 terminal」）。
7. **不混计数**：同一 effect 的 request、claim、outcome 计数不得混成 exactly-once——exactly-once 是 Gate I 目标，F 只负责「每个状态迁移 fail-closed 闭包」，不承诺外部幂等。

**门禁**：只有 effective input 已在正确 guardrail/cache/dispatch 边界 durable 记录，且用户单独批准 Safe Replay gate（I）后，才允许实现真实 replay invocation。F 不越过该门禁。

### 3.4 Gate G — committed projection/subscription wiring

**目标**：把纯 `CapabilityChangeStream` 接到真实 source。

**现有事实**：`ToolHandlerRegistry` 已有 committed change feed —— `RegistryShared.change_tx: broadcast::Sender<RegistryChange>`（`src/tools/registry.rs:108`），`mutation_lock`（`src/tools/registry.rs:123`）序列化 ArcSwap 与 change 广播，保证订阅者按 revision 顺序观察事件；`subscribe()` 返回 `broadcast::Receiver<RegistryChange>`（`src/tools/registry.rs:541`）。

设计约定：

1. **atomic snapshot + cursor capture**：`subscribe(scope, cursor)` 先取 `descriptor_snapshot()` + `revision()`，再建 `broadcast::Receiver`；用 cursor 裁剪 `RegistryChange`。
2. **registry mutation 与 committed event 的顺序**：依赖现有 `mutation_lock` 保证；不得绕过。若 future plan 引入跨 store 的 committed event（非本 registry 的 in-process broadcast），须重新评估顺序保证——本 spec 明确 **registry 的 in-process broadcast 不是 recovery source**，recovery 只认 `SessionEventStore`。
3. **owner revoke / generation bump 的 Invalidated 语义**：`OwnershipTree::bump/revoke` 触发 `CapabilityChange::Invalidated`，保持 cursor 不回零。
4. **snapshot+delta resync 去重**：复用 `CapabilityChangeStream::invalidate_and_resync`（`src/capability/facade.rs`）的 `(id, revision)` 去重。
5. **overflow 语义**：`broadcast` 落后（lag）时不得伪装成完整逐条投递——发最新 snapshot + 新 cursor（`Invalidated` 语义），不能假装逐条送达。
6. **GlobalBus 仍只做低延迟通知**：不把 GlobalBus 当 change feed。
7. **projection 不成为 recovery source**：StateDatabase / ACP JSON / MCP projections 不能授权、不能 resolve handler、不能决定 replay、不能维护第二份 identity/revision。为每个 projection（ACP/MCP/model）分别证明这四点。

---

## 4. 数据流与 ownership

### 4.1 层级

```
Runtime → Session → Run → Task → EffectClaim
```

- `OwnershipTree`（`src/capability/ownership.rs`）已实现 register/resolve/claim/claim_state/bump/revoke/dispose；`LifetimeScope` rank：`External < Task < Run < Session < Runtime`；`bump(scope)` 重写 at-or-below scope 的 binding generation 并清空其 claims；`revoke`/`dispose` 不可逆。
- `VisibilityScope` 是正交 ACL 轴（principal/workspace/session/allowed_kinds/allowed_ids/approval），与 `LifetimeScope` 独立。
- `EffectClaim`（`src/capability/ownership.rs`）是「一个 request 对一个 fence 的 ownership 断言」，`FencingToken` 只有 `FencingToken::new(nonce)` 一个构造函数（owner 层专属）。

### 4.2 跨 crate wire schema 单源派生

- `SessionEvent::EffectClaim*` 的 wire 身份是 `(request_id, owner, owner_generation, fence)`，由 `src/session/events.rs` 单一定义；`src/capability/effect_claim.rs::EffectClaimEvent` 是它的 reducer 投影，不是第二份契约——它从 `SessionEvent` 派生（`effect_claim_event_from_session`），不重复定义身份字段来源。
- 判据 #10（跨 crate wire 契约两边各持一份形状互相抵消）：任何新增 wire schema（若 E 引入 schema fingerprint 的 canonical form）必须由单一 canonical source 派生，descriptor 与 projection 不得各持一份。
- 判据 #12（顺序/单位/边界同一处派生）：cursor/revision 语义统一派生自 `ToolHandlerRegistry::revision()` 与 `SessionEventStore` seq，不得另起计数器。

---

## 5. 错误语义、幂等边界、并发/顺序、fail-closed

### 5.1 错误语义

- `reconcile_effect_claim` 对任何非法输入返回 `EffectClaimState::Unknown`（fail-closed），不返回「成功」。
- `OwnershipTree::claim` 在 revoked / disposed / owner 不匹配时返回 `None`；`claim_state` 对不存在/被 bump 的 claim 返回 `ClaimState::Unknown`。
- `SessionService::emit_batch` 默认返回 `Err`（`"this SessionService cannot append atomically"`），只有 `InProcessActorSessionService` 提供原子 override——这本身就是「不能原子提交就说不能，而非报一个会撕裂的成功」的 fail-closed 形状。
- `SessionEventStore::append_batch` 空 events 且无 retire 被拒绝（「报成功的 no-op」判据 #11 的防御）。

### 5.2 幂等边界

- `CapabilitySlot::install` 幂等（第二次调用返回 `false`）；`RegistrationHandle::dispose` 幂等（generation-guarded，重复调用 `false`）。
- `SessionEventStore` 的 `(session_id, seq)` 主键保证重复 append 冲突失败而非重复落盘。
- effect claim 的幂等边界在 F：reducer 对 duplicate active claim / terminal 后再迁移 → `Unknown`，**不**做「第二次调用等于第一次」的语义承诺——外部 effect 幂等属 Gate I。

### 5.3 并发 / 顺序

- `ToolHandlerRegistry` 的 `mutation_lock` 序列化「ArcSwap 状态交换 + change 广播」，保证 revision 与 change event 同序发布（`src/tools/registry.rs:111-119` 注释明确这是防 `swap(rev=2); swap(rev=1); send(rev=1); send(rev=2)` 交错）。
- `CapabilityChangeStream` 只接受严格单调 cursor（`append` 对 `cursor <= committed_cursor` 拒绝），`invalidate_and_resync` 对 `snapshot.committed_cursor < old` fail-closed 不突变。
- `SessionService::emit_batch` 是唯一原子多事件提交路径；并发 writer 靠 store 事务与 seq 主键串行化。

### 5.4 fail-closed 清单

以下场景全部 fail-closed，不得「报成功」：

| 场景 | fail-closed 行为 |
| --- | --- |
| malformed claim identity（空 request_id/owner、0 generation、0 fence） | reducer → `Unknown`（`src/capability/effect_claim.rs` 已测） |
| duplicate active claim | reducer → `Unknown` |
| terminal 后再次迁移 | reducer → `Unknown` |
| stale owner generation（lease） | E：resolve 拒绝 / lease 判定 stale，不返回 handler |
| schema mismatch | registry `register/replace` 校验 `matches_definition` 失败 → 拒绝 |
| 外部 effect 不可观测 | F：`Terminal` 缺失 → `Unknown` |
| snapshot 落后于 subscriber cursor | `invalidate_and_resync` 返回 `false` 不突变 |
| emit_batch 无法原子提交 | `Err`，不假装成功 |

---

## 6. 测试与验证矩阵

> 验证命令基线（来自上游 prompt，须在 canonical checkout 或后续 worktree 显式 `cd` 后运行）。ACP conformance 仅作为后续 fixture 参考，不把 H 当作 E–G 的验收项。

| 维度 | 验证内容 | 测试归属 |
| --- | --- | --- |
| 类型/编译 | `cargo check -p alephcore` | E/F/G 每 gate |
| facade 纯值状态机 | `cargo test -p alephcore --lib capability::facade`（现有 5 passed） | 基线回归 |
| effect claim reducer 闭包 | `cargo test -p alephcore --lib capability::effect_claim`（现有 16 passed） | F 基线 |
| session events 序列化 | `cargo test -p alephcore --lib session::events`（现有 28 passed） | 基线 |
| approval 原子批 | `cargo test -p alephcore --lib session::in_process::tests::approval_memo_batch_is_one_contiguous_commit`（1 passed） | F 事务边界 |
| metadata wrapper | `cargo test -p alephcore --lib tools::service::metadata_form_tests`（6 passed） | 判据 #17 / wrapper 语义 |
| scoped dispatch | `cargo test -p alephcore --lib tools::scoped::tests`（130 passed） | E producer 候选路径 |
| hooks | `cargo test -p alephcore --lib extension::hooks`（193 passed） | HookMemo 路径 |

**E 新增测试**：
1. snapshot/handler/descriptor 同代一致性（describe 与 dispatch 用同一代）。
2. stale lease 判定（owner generation 落后 → 拒绝，不返回 handler）。
3. replace/unregister 后旧 lease fail-closed。
4. projection 与 dispatch 同代（projection 不旁路 registry）。
5. change cursor 恢复且无重复（断线重连 `(id, revision)` 去重）。
6. schema fingerprint 由单一 owner 计算且稳定（同一 schema 两次 fingerprint 一致）。

**F 新增测试**（若实现 producer）：
1. EffectClaim replay/recovery：从 `SessionEventStore` 读回 claim 事件，`reconcile_session_events` 与真实 event 一致。
2. 每个 crash 窗口（intent/claim/invocation/outcome）的 reducer 终端态断言。
3. 外部 effect 不可观测 → `Unknown`（不推断成功）。
4. 「报成功的 no-op」负例：空 events 空 retire 被拒。

**G 新增测试**：
1. projection ordering：mutation 顺序与 change event 顺序一致（依赖 `mutation_lock`）。
2. reconnect/overflow：落后订阅者收到 snapshot+新 cursor，不伪装逐条投递。
3. ACP/MCP/model projection 四点证明（不能授权 / 不能 resolve handler / 不能决定 replay / 不维护第二份 identity/revision）——以 conformance fixture 形式记录，**不把 H 的完整 ACP server 当作 E–G 验收项**。

**禁止**：`cargo fmt --check` 全仓运行会被既有无关文件格式漂移阻断；改动文件须单独核验格式，报告区分 baseline drift 与本次变更。

---

## 7. 旧代码清理与文档更新清单

| 项 | 动作 | 说明 |
| --- | --- | --- |
| `src/capability/backend.rs::to_descriptor` 的 `fingerprint: [0u8; 32]` 占位 | 由 E 替换为 describe/projection 路径计算 | 关闭「未计算 fingerprint」gap |
| `src/capability/backend.rs::ToolBackendAdapter::generation()` 常量 `OwnerGeneration(0)` | 由 E 接管为 `OwnershipTree` 真实 generation | 关闭占位，避免 descriptor 自构造即 stale |
| `docs/reference/FEATURE_LOCATOR.md` §3.5d | 更新为「E/F/G 已落地」状态（或「部分落地 + deferred」） | 明确哪些 deferred 仍成立 |
| `docs/reference/ARCHITECTURE.md` / `DESIGN_PATTERNS.md`（如涉及） | 补 authority/projection/notification 三类边界 | 与 §3.1 对齐 |
| `docs/superpowers/specs/2026-10-06-capability-phase4-design.md` §12 非本期范围 | 交叉引用本 spec | 保持上下游一致 |
| `AGENTS.md` 的「Capability Phase 4 入口」段 | 同步 E/F/G 结论（改 `main` 前先核验） | 与 `CLAUDE.md` 同步 |
| 空引用 `deepseek-harness/packages/capability/` | 从 gap analysis / 文档中剔除 | 实际不存在（§2.1） |
| 无第二套 registry/ownership 残留 | review diff 检查 | 判据 #16 孪生子系统防御 |

---

## 8. 实施顺序与双线并行依赖

### 8.1 顺序

1. **Gate E**（先）——它是 F/G 的前置：F 的 producer 需要 E 的 concrete facade 提供「真实 owner generation + lease」，G 的 committed feed 需要 E 的 `subscribe` 落地。
2. **Gate F**（design review → 决定是否实现 producer）——依赖 E 的 ownership/lease 语义；不阻塞 G。
3. **Gate G**（可与 F 双线并行）——依赖 E 的 `subscribe` + registry change feed，与 F 无直接数据依赖。

### 8.2 双线并行

- 线 1：E → F（recovery/producer 线）。
- 线 2：E → G（projection/subscription 线）。
- E 完成后 F 与 G 可并行；二者共享「`ToolHandlerRegistry` change feed」「`OwnershipTree`」只读依赖，无写入冲突，但须避免同一文件（`facade.rs` / `backend.rs`）的编辑竞争——plan 阶段应把共享文件改动归入 E，F/G 只新增各自模块或测试。

### 8.3 低内存时 cargo 启动前检查

- 低内存环境跑 `cargo check/test` 前，先确认 `src/capability/` 与 `src/session/` 相关 crate 目标范围（`-p alephcore`），避免全 workspace 构建触发高内存。
- 启动前先 `git status --short --branch` 确认 worktree 干净、HEAD 与上游基线一致，再 `cd` 到对应 worktree 显式执行；不依赖 shell 默认 cwd。
- 建议按 gate 拆分 `cargo test -p alephcore --lib <mod>` 定向运行，而非全量，减少峰值内存与编译时间。

---

## 9. 装配点（implementation plan 阶段由源码确认）

以下点本 spec **不发明具体类型/函数名**，标注为待 plan 阶段用现有源码核验：

1. `Zahir` concrete impl 的类型名与落点（本文档暂记 `ConcreteZahir`）。
2. `resolve` 的 stale 判定放在 facade 层还是 backend 层（`BackendLease` 现有字段只有 `descriptor + owner_generation`，无 stale 标志）。
3. F producer 的真实生产路径清单（scoped dispatch / MCP face 具体哪个函数发出 claim 事件）。
4. schema fingerprint 的 canonical schema serialization owner（descriptor 还是 describe/projection 路径，谁持有 canonical form）。
5. E 中 `describe` 的 `committed_cursor` 是否直接取 `revision()`，还是需要独立 cursor 计数器（判据 #12：顺序/边界同一处派生）。

---

## 10. 附录：关键源码锚点

- `src/capability/descriptor.rs` — `CapabilityKind`（9 种）、`CapabilityDescriptor`、`CapabilityRevision(u64)`、`SchemaRef { version, fingerprint: [u8;32] }`。
- `src/capability/facade.rs` — `Zahir` trait（`describe/resolve/subscribe/project`）、`BackendLease`、`CapabilityChangeStream`、`CapabilityChange`、`Cursor`。
- `src/capability/backend.rs` — `CapabilityBackend` trait、`ToolBackendAdapter`、`to_descriptor`（fingerprint 未计算）、`TOOL_NAMESPACE = "aleph/tools"`。
- `src/capability/ownership.rs` — `OwnerGeneration(u64)`、`LifetimeScope`、`VisibilityScope`、`OwnerRef`、`FencingToken`、`EffectClaim`、`OwnershipTree`（register/resolve/claim/claim_state/bump/revoke/dispose）、`ClaimState`、`RegisterError`。
- `src/capability/effect_claim.rs` — `EffectClaimState`、`EffectClaimEvent`、`effect_claim_event_from_session`、`reconcile_effect_claim`、`reconcile_session_events`。
- `src/capability/mod.rs` — `CapabilitySlot`、`MissingSemantics`、`Outcome`。
- `src/session/events.rs:710-756` — `SessionEvent::EffectClaimPrepared/Claimed/Invoking/Terminal`、`ApprovalMemo`、`HookMemo`（`Durability::Normal`）。
- `src/session/store.rs` — `SessionEventStore` trait（`append_batch` 单事务）、`SqliteEventStore`、`(session_id, seq)` 主键。
- `src/session/service.rs` — `SessionService` trait、`emit_batch` 默认拒绝、`InProcessActorSessionService` 唯一生产 override。
- `src/session/replay.rs` — `ReplayPermit`（`pub(crate) new`、single-use）、`ReplayPrepare::Ready/Refused`、`ReplayPreparer`、`REPLAY_MAX_ATTEMPTS_PER_CALL = 3`。
- `src/tools/registry.rs` — `ToolHandlerRegistry`、`RegistryChange`、`RegistryShared { inner: ArcSwap, change_tx: broadcast::Sender, mutation_lock }`、`register/replace/unregister/descriptor/descriptor_snapshot/revision/close/subscribe`。
- `src/tools/service.rs:453` — `to_metadata_form(defs: &[ToolDefinition]) -> Arc<[ToolDefinition]>`（compatibility wrapper）。
