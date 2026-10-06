# Capability Phase 4 后续 Gate（E→F→G）— 架构书面 spec

| 项目 | 取值 |
| --- | --- |
| 文档性质 | 架构书面 spec（非已实现代码；不修改产品代码，不提交） |
| 适用分支 | `capability-phase4-follow-up` |
| 范围基线 | 已批准「实现 Gate E→F→G，Gate H/I 延后」 |
| 文档基线 | `1ea05b1ce86d35b64384d8157f015fd68224cdf6`（`main` Phase 4 基座） |
| 上游 spec | `docs/superpowers/specs/2026-10-06-capability-phase4-design.md` |
| 上游 prompt | `docs/superpowers/prompts/2026-10-06-capability-phase4-follow-up-gates.md` |
| 批准记录 | 用户 m00430 确认升级 spec/plan 后继续 |
| 日期 | 2026-10-06 |

> 本文档只描述设计意图，不修改任何源码。文中所有 `src/...` 路径用于定位既有符号；「现有 / 已实现」描述源码事实（截至文档基线提交），「应当 / 将 / 新增」为设计约定（本 spec 已批准的升级设计，不再是待定项）。凡**现有源码事实**与本文档冲突，以源码为准；凡本文档标注「新增 / 设计约定」的契约，是 implementation plan 要落地（而非要重新决定）的批准设计。

---

## 1. 目标、成功标准与非目标

### 1.1 目标

在 Phase 4 基座（Gate A–D 已合并）之上，把「纯 contract + 纯 value 状态机」接到真实运行时，分三步落地：

- **Gate E**：concrete `Zahir` facade/backend wiring —— 把 `ToolBackendAdapter` 与 `OwnershipTree` 组合成唯一 Tool facade 的 live backend；`resolve` 改为 `Result<BackendLease, ResolveError>`，新增 `validate_lease` 作为唯一 use-time stale 校验入口；不建立第二套 registry，不复制 descriptor/handler 事实。
- **Gate F**：EffectClaim producer 与 recovery integration —— 依赖 per-binding owner generation 与 `validate_lease`；钉死 claim / budget reservation / owner generation / fencing token 的事务边界与每个 crash 窗口；producer 只经原子 `SessionService::emit_batch` / `SessionEventStore::append_batch` 提交，不实现 automatic replay。
- **Gate G**：committed projection/subscription wiring —— 把纯 `CapabilityChangeStream` 接到真实 registry change feed（`ToolHandlerRegistry::subscribe` 的 live broadcast），并证明 ACP/MCP/model projection 不能授权、不能 resolve handler、不能决定 replay、不能维护第二份 identity/revision。

### 1.2 成功标准（验收判据）

1. **同代一致性**：snapshot / handler / descriptor 三者在同一 atomic registry snapshot 上派生（`describe` 与 dispatch 用同一代）。禁止 describe 走 `descriptor_snapshot()`、dispatch 走 `resolve()` 导致代际撕裂。
2. **resolve 返回 `Result`**：`Zahir::resolve` 返回 `Result<BackendLease, ResolveError>`；lookup 未命中 / 不可见 / 已 revoke 时返回 `Err`，不得返回一个「看起来可用」的 lease。
3. **stale lease 只经 `validate_lease` fail-closed**：`Zahir::validate_lease(&self, lease) -> Result<(), ResolveError>` 是唯一 use-time stale 校验入口；消费者不得自行比较 `lease.owner_generation` 与任何 generation 来源；`BackendLease` 自身不携带任何 validity 状态（无 `fence` / `is_valid` / `invalidate` / `StaleLease`）。
4. **无第二套 registry**：E/F/G 全程不新增 handler map / capability map；`ToolHandlerRegistry` 仍是 callable Tool 唯一事实源；`OwnershipTree` 是唯一 ownership 事实源（不旁路它自造 owner 计数）。
5. **generation 权威只在 lease + `OwnershipTree`**：owner generation 是 `(CapabilityId, VisibilityScope)` 级，经 `OwnershipTree::generation()` 只读读出；不是全局 registry revision；`CapabilityBackend::generation()` 与 `CapabilityDescriptor.owner_generation` 的设计承诺删除。
6. **projection 与 dispatch 同代**：projection 只读 canonical snapshot，不旁路 registry，不重算 fingerprint。
7. **cursor 单调且无重复；recovery 只认 durable store**：`CapabilityChangeStream` 的 cursor 严格单调、`(CapabilityId, CapabilityRevision)` 去重、永不从零开始；registry broadcast 落后时以 snapshot + 新 cursor 重同步（`Invalidated` 语义），不伪装逐条投递。registry broadcast **无 durable cursor**，断线/重启后的 cursor 恢复只从 `SessionEventStore` 读（§3.4），不声称 broadcast 可断线恢复。
8. **fail-closed 全覆盖**：Unknown / missing identity / NotVisible / Revoked / StaleOwner / schema mismatch / invalid claim 一律 fail-closed（§5.4）。
9. **不变量不破坏**：上游 prompt 列出的 10 条当前不变量全部保持（§1.4）。

### 1.3 非目标（明确排除，本期不实现）

| 非目标 | 归属 gate | 说明 |
| --- | --- | --- |
| 完整 ACP server gate | H（延后） | 不审计 inbound command→Session/Run mutation line、不实现 ACP transport/session manager 全链路；ACP JSON 已有 projection 不构成「ACP server 完成」的证据 |
| automatic Safe Replay / external-effect exactly-once | I（延后） | 不实现真实 replay invocation；`ReplayPermit` 保持 `pub(crate)`、single-use、`effective_input: None` 生产侧无条件拒绝 |
| universal durable scheduler | I（延后） | 不引入任何 scheduler |
| external-effect handling | I（延后） | 外部 effect 不可观测时 reducer 落 `Unknown`，不推断成功、不实现外部幂等 |
| 其余 8 类 kind backend | 未定 gate | Skill/Agent/Task/Resource/EventSource/Subscription/Plugin/Hook 仍只有 contract/deferred，不挂载 |
| `CapabilitySlot` 动态化 | 禁止 | 保持启动期 first-writer-wins |
| `src/harness/` 扩张 | 禁止（R10） | 不新增 harness 业务文件承载 facade 业务 |
| 通过 `to_metadata_form` 生成 canonical identity | 禁止 | 仍只做旧 Tool metadata compatibility wrapper |
| batch-id durable set / duplicate-ignore | 禁止 | 不用「已提交 batch-id 集合」做幂等；幂等由 store `(session_id, seq)` 主键与 reducer fail-closed 保证 |
| 把 registry broadcast 当 recovery source | 禁止 | broadcast 是 live 通知，`SessionEventStore` 才是 durable recovery authority（§3.4） |

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

本期升级追加的**新不变量**（同样不得破坏）：

11. **per-binding generation**：owner generation 是 `(CapabilityId, VisibilityScope)` 级，不是全局计数器；`OwnershipTree::generation(&CapabilityId, &VisibilityScope) -> Option<OwnerGeneration>` 只读返回单个 binding 的 generation。
12. **generation 权威单源**：`BackendLease.owner_generation` + `OwnershipTree::generation()` 是 owner generation 的唯一权威；`CapabilityDescriptor.owner_generation` 与 `CapabilityBackend::generation()` 不作为权威（设计承诺已删除）。
13. **唯一 stale 校验入口**：`Zahir::validate_lease` 是唯一 use-time stale 校验入口，消费者不得自行比较 generation。
14. **fingerprint 单源**：`to_descriptor` 是 `SchemaRef.fingerprint` 的唯一 owner；describe/projection 不重算，不复用 harness 参数 canonicalizer。
15. **live broadcast ≠ durable recovery**：`ToolHandlerRegistry::subscribe` 的 broadcast 是 live 通知（256 槽、lag 可丢），不是 durable recovery source；durable recovery 只认 `SessionEventStore`。

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

**concrete tool host 的职责**（本 spec 已批准的设计，不再是「待定」）：

concrete `Zahir` impl（下文记 `ConcreteZahir`，其具体类型名由 implementation plan 落一个名字，但**职责与字段已钉死**）组合三类既有组件，不复制任何 registry：

- 持有 `ToolBackendAdapter`（内含 `ToolHandlerRegistry`，callable handler 唯一事实源）；
- 持有 `Arc<OwnershipTree>`（owner generation / revoke / dispose 唯一事实源）；
- 持有初始化用的 registry snapshot（启动期一次），用于向 `OwnershipTree` 注册 runtime/default binding。

`ConcreteZahir` 不持有第二份 descriptor map / handler map / owner 计数。descriptor 与 handler 都从 `ToolHandlerRegistry` 现有 snapshot 派生；owner generation 只从 `OwnershipTree` 读。

**registry 动态变更、初始 binding、re-registration 的可执行不变量**（见 §3.2 E8，全文无 TBD）。

### 3.2 Gate E — concrete facade/backend wiring

**目标**：`ConcreteZahir` 持有 Tool backend + `Arc<OwnershipTree>`，不复制 descriptor/handler truth；`resolve` 返回 `Result<BackendLease, ResolveError>`，`validate_lease` 是唯一 use-time stale 校验入口。

#### E1. `Zahir::resolve` 返回 `Result`

`src/capability/facade.rs:222-226` 现有 trait 的 `resolve(&self, reference: Reference) -> BackendLease`（按值返回）改为：

```rust
pub trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError>;
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError>;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}
```

「无 lease」与「stale」不再用「返回一个空/带标记的 lease」表达，而是 fail-closed 返回 `Err(ResolveError)`。resolve 只在拿到合法 binding 时返回 `Ok(lease)`。

#### E2. `validate_lease` 是唯一 use-time stale 校验入口

- `fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError>`：消费方在**使用** lease 前必须调用它，通过即 `Ok(())`，不通过即 `Err(ResolveError)`（fail-closed）。
- **消费者不得自行检查 generation**：任何「把 `lease.owner_generation` 与当前 generation 比较」的判断都属于该入口的职责；consumer 只调 `validate_lease`，不读 `OwnershipTree`、不自比 `OwnerGeneration`。
- `validate_lease` 内部三道 fail-closed 判据，**不接收任何外部 `Reference`/`VisibilityScope` 参数**（binding 完全由 lease 自带的 descriptor 定位）：
  1. **binding 定位**：以 `lease.descriptor.id` 与 `lease.descriptor.visibility`（见 E7）为键，`OwnershipTree::generation()` 读该 binding。binding 缺失 ⇒ 已 revoke 归 `Revoked`，否则归 `Unknown`。
  2. **owner generation**：`lease.owner_generation != 当前 per-binding generation` ⇒ `Err(ResolveError::StaleOwner)`。
  3. **descriptor freshness**：以 `lease.descriptor.id` 重新 `lookup` 当前 registry descriptor，比较 `(id, revision, schema.fingerprint)`；任一不一致 ⇒ `Err(ResolveError::Unknown)`（旧 descriptor 不得继续作为当前可调用 capability 的凭证，见 E8 不变式 3）。

#### E3. `ResolveError` 闭集变体

```rust
/// 新增（设计约定）：resolve / validate 的闭集错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// lookup 未命中、binding 缺失、或任何无法归类的失败 —— fail-closed。
    Unknown,
    /// `resolve` 时调用方 `Reference.visibility` 对目标 binding 不可见。
    NotVisible,
    /// binding 已 revoke。
    Revoked,
    /// lease 的 owner_generation 落后于当前 per-binding generation。
    StaleOwner,
}
```

**resolve-time 与 validate-time 错误矩阵**（闭集，不新增其它变体）：

| 变体 | `resolve` 可出现 | `validate_lease` 可出现 |
| --- | --- | --- |
| `Unknown` | 是（lookup 未命中、binding 缺失、id 不属于 backend） | 是（binding 已被 dispose/移除；或 descriptor freshness 不一致——旧 lease 的 `(id, revision, schema.fingerprint)` 与当前 registry descriptor 不匹配） |
| `NotVisible` | 是（`reference.visibility` 无对应 binding） | **否**（validate 以 lease 自带的 `descriptor.visibility` 为键，无调用方 visibility） |
| `Revoked` | 是（binding 已 revoke） | 是（binding 在 lease 发放后被 revoke） |
| `StaleOwner` | **否** | 是（`lease.owner_generation` ≠ 当前 per-binding generation） |

**关键约束**：`StaleOwner` 禁止写成 `resolve` 必然可以产生的错误。`resolve` 返回的 lease 是**当场铸造**的——`lease.owner_generation` 取自 resolve 时刻的 per-binding generation，因此新 lease 在构造点不可能 stale。`StaleOwner` 只可能出现在 `validate_lease`（对一个**先前发放、之后被 bump/revoke** 的 lease 做 use-time 校验时）。同理，descriptor freshness（`Unknown`）也是 validate-only：`resolve` 当场 `lookup` 的 descriptor 就是当前 descriptor，构造点不可能 freshness 失败，它只在 replace/unregister 之后对旧 lease 触发。实现与测试都必须遵守这个不对称性。

#### E4. `BackendLease` 保持两字段

`src/capability/facade.rs:55` 的 `BackendLease` 保持**仅**：

```rust
pub struct BackendLease {
    pub descriptor: CapabilityDescriptor,
    pub owner_generation: OwnerGeneration,
}
```

明确**不加**以下字段/方法/变体：`fence`、`is_valid`、`invalidate`、`StaleLease`。lease 是纯 value：它的 stale 状态不在 lease 上编码，而是由 `validate_lease` 对 `OwnershipTree` 现场判定。任何「给 lease 加 fence / validity 标志 / 自失效方法」的设计都违反该约定。

#### E5. `OwnershipTree` 新增只读 per-binding `generation` 契约

`OwnershipTree`（`src/capability/ownership.rs:216`）现有 `register/resolve/claim/claim_state/is_revoked/is_disposed/bump/revoke/dispose`。其中 `resolve(&CapabilityId, &VisibilityScope) -> Option<()>`（`:272`）只返回存在性；`bump`（`:339`）会**变更**并返回 generation。缺口是「无变更地读单个 binding 的当前 generation」。

新增（设计约定，只读、不 bump nonce）：

```rust
/// 只读返回 `(CapabilityId, VisibilityScope)` 这个 binding 的当前 owner generation。
/// 无该 binding 时返回 `None`。不修改 nonce / generation / claims。
pub fn generation(
    &self,
    capability: &CapabilityId,
    visibility: &VisibilityScope,
) -> Option<OwnerGeneration>;
```

契约要点：

- generation 是 **`(CapabilityId, VisibilityScope)` 级**，与现有 binding key（`src/capability/ownership.rs:233` 的 `key()`，visibility 参与 key）一致。
- **不能是全局 registry revision**：registry 的 `revision()`（raw `u64`，工具注册变更计数）与 owner generation 无关，禁止把 `generation()` 实现成读 registry revision。
- `bump(scope)` 重写 at-or-below scope 的 binding generation 后，`generation()` 读到新值；`revoke(capability)` / `dispose(scope)` 移除 binding 后，`generation()` 返回 `None`。

#### E6. 删除 `CapabilityBackend::generation()` 与 `CapabilityDescriptor.owner_generation` 的设计承诺

- **`CapabilityBackend::generation()` 设计承诺删除**：`src/capability/backend.rs:25` trait 现有的 `fn generation(&self) -> OwnerGeneration` 与 `ToolBackendAdapter` 的常量实现（`src/capability/backend.rs:73` 返回 `OwnerGeneration(0)`，注释「Task 2 替换此常量」）一并删除，不再作为契约。owner generation 的读取只走 `OwnershipTree::generation()`。backend 层不再承担「当前 owner generation」的承诺。
- **`CapabilityDescriptor.owner_generation` 设计承诺删除**：`src/capability/descriptor.rs` 的 `CapabilityDescriptor` 现有 `pub owner_generation: OwnerGeneration` 字段不再承诺为权威 generation。generation 权威只在 `BackendLease.owner_generation` + `OwnershipTree::generation()`；descriptor 上的该字段（若实现保留）不得被任何 stale 判定或授权逻辑读取。

#### E7. `Reference` / `Scope` 都携带 `VisibilityScope`

`src/capability/facade.rs:18` 的 `Scope` 与 `:27` 的 `Reference` 都新增 `visibility: VisibilityScope` 字段：

```rust
pub struct Scope {
    pub kind: Option<CapabilityKind>,
    pub namespace: Option<String>,
    pub visibility: VisibilityScope,   // 新增
}

pub struct Reference {
    pub id: CapabilityId,
    pub visibility: VisibilityScope,   // 新增
}
```

- **facade 不自行猜测调用者身份**：`resolve` 用 `reference.visibility`、`describe`/`subscribe`/`project` 用 `scope.visibility` 为键；`validate_lease` **不接收** visibility，用 lease 自带的 `descriptor.visibility` 为键。`Reference.visibility` 只用于 `resolve` 定位 binding，不得在 facade 内部凭空构造一个 visibility 代替调用者。
- **`VisibilityScope::default()`（全 `None` = 全域可见）只是当前无 ACL 上下文**：它表达「当前没有 ACL 数据、默认可见」，不是「facade 知道调用者是谁」。引入真实 ACL 时，调用方负责传入正确的 visibility，facade 的键化逻辑不变。

#### E8. concrete tool host 职责与可执行不变量

`ConcreteZahir` 组合 `ToolBackendAdapter` + `ToolHandlerRegistry` + `Arc<OwnershipTree>`，不复制 registry。以下不变量全部可执行、可测试：

1. **初始 binding（runtime/default）**：host 构建时，用 `ToolBackendAdapter::enumerate(&Scope { visibility: VisibilityScope::default(), .. })` 或 `ToolHandlerRegistry::descriptor_snapshot()` 遍历当前工具，对每个工具的 `(CapabilityId { namespace: TOOL_NAMESPACE, name }, VisibilityScope::default())` 注册一次（启动期对账，不随每次变更重复执行）。这是 host 层的**对账契约**，不是 `OwnershipTree::register` 的语义：`register` 对同 key `(CapabilityId, VisibilityScope)` 是**覆盖写入**（`HashMap::insert`），不是 first-writer-wins（`CapabilitySlot` 才是 install-once，二者不得类比）。host 若需判断「binding 已存在」，spec 只规定最终不变量（见下），具体 helper 由 implementation plan 确定。
2. **registry 动态变更不改变 owner generation**：`ToolHandlerRegistry::register/replace/unregister` 只改 handler/descriptor 事实并推进 registry revision（raw `u64`）；**不** bump `OwnershipTree` 的 generation，也**不**把 registry revision 当 owner generation。owner generation 只由 `OwnershipTree::bump/revoke/dispose` 推进。
3. **re-registration（replace）语义**：对已绑定工具 replace，descriptor/handler 由 registry 更新（下次 `resolve` 重新 `lookup` 取新 descriptor），ownership binding 保持既有 generation **不重置**（host 不因 replace 重新 `register`）。owner generation 校验对旧 lease 仍通过（generation 未 bump），**但** descriptor freshness 判据使旧 lease fail-closed：旧 lease 的 `(id, revision, schema.fingerprint)` 与当前 registry descriptor 不一致 ⇒ `Err(ResolveError::Unknown)`。因此旧 lease 在 replace 后**不得继续作为当前可调用 capability 的凭证**，只有重新 `resolve` 铸造的新 lease 才可用。owner generation 与 descriptor freshness 是两道正交判据（前者 `StaleOwner`、后者 `Unknown`），二者都不读 registry `revision()` 作为 owner generation。
4. **unregister 语义**：`unregister` 后 `ToolHandlerRegistry::descriptor()` 返回 `None` ⇒ `resolve` 的 lookup 未命中 ⇒ `Err(ResolveError::Unknown)`（fail-closed）；已发旧 lease 在 `validate_lease` 时因 lookup 未命中（descriptor freshness）⇒ `Err(ResolveError::Unknown)`。unregister **不得**调用 `OwnershipTree::revoke`（不可逆）：它只影响 descriptor lookup/validation，不撤销 ownership，因此工具后续 **re-register** 被允许（host 对该 `(CapabilityId, VisibilityScope)` 重新 `register`，`resolve` 恢复可用）。
5. **resolve/validate 读唯一 ownership source**：`resolve` = `CapabilityBackend::lookup`（取 descriptor）+ `OwnershipTree::generation()`（取 per-binding generation）+ 组装 `BackendLease { descriptor, owner_generation }`；`validate_lease` = `OwnershipTree::generation()` 比较 `lease.owner_generation`（`StaleOwner`）+ `CapabilityBackend::lookup` 比较 descriptor freshness（`Unknown`）。二者都不读 registry `revision()` 作为 generation。
6. **registry revision 与 owner generation 分离**：`CapabilitySnapshot.committed_cursor`（`Cursor(u64)`）取 registry `revision()`（`src/tools/registry.rs:501`）；`BackendLease.owner_generation` 取 `OwnershipTree::generation()`。二者语义、来源、计数互不混用（判据 #3、#12）。
7. **不复制 registry**：host 不维护第二份 descriptor/handler/owner map；`OwnershipTree` 只存 ownership 事实（owner/lifetime/generation/claims），不存 handler 或 descriptor。
8. **`Arc<OwnershipTree>` 单实例共享**：bump/revoke/dispose 的写入方与 resolve/validate 的读取方共享同一 `Arc<OwnershipTree>`，保证「写到的 generation」与「读到的 generation」是同一份，杜绝第二份 owner 计数（判据 #16 孪生子系统防御）。

#### E9. `to_descriptor` 是唯一 schema fingerprint owner

- `src/capability/backend.rs:92` 的 `fn to_descriptor(&ToolCapabilityDescriptor) -> CapabilityDescriptor` 是 `SchemaRef.fingerprint` 的**唯一** owner。当前 `src/capability/backend.rs:103` 的 `fingerprint: [0u8; 32]` 占位在此处替换为真实 digest。
- **使用 workspace 已有 sha2 依赖**：`Cargo.toml:113` 与 `:261` 的 `sha2 = "0.10"`，不新增依赖。
- **canonical schema digest**：`Sha256` over `ToolCapabilityDescriptor.input_schema` 的 canonical 序列化（稳定键序、无无关空白），写入 `SchemaRef.fingerprint`（`[u8; 32]`）。
- **projection/describe 不重算**：`describe`/`project`/`CapabilityBackend::enumerate` 复用 `to_descriptor` 产出的 `SchemaRef.fingerprint`，不再各自算一份（判据 #1「同一事实的两份表述」防御）。
- **不复用 harness 参数 canonicalizer**：harness 自有的参数 canonicalization 与 capability schema 的 canonical form 是两套契约，capability 侧用 `to_descriptor` 内的单源 canonical 序列化，不借用 harness 的 canonicalizer（判据 #12）。

**禁止**（同上游 Gate E）：`dyn Any`、巨大 payload enum、第二套 handler map、把 `CapabilityRevision` 与 `OwnerGeneration` 合并、`CapabilitySlot` 动态化、改 `src/harness/`、经 `to_metadata_form` 生成 canonical identity、给 `BackendLease` 加 `fence`/`is_valid`/`invalidate`/`StaleLease`、发明 `Zahir::list()`。

### 3.3 Gate F — EffectClaim producer 与 recovery integration

**目标**：先做 design review，再决定是否实现 producer。本 spec 钉死 producer 的语义边界，不承诺实现 automatic replay。

**现有 reducer/adapter 事实**（`src/capability/effect_claim.rs`）：
- `effect_claim_event_from_session(&SessionEvent) -> Option<EffectClaimEvent>`（`:31`）：把 4 个 claim 事件（`Prepared/Claimed/Invoking/Terminal`）转成 reducer 输入；非 claim 事件被忽略。
- `reconcile_effect_claim(&[EffectClaimEvent]) -> EffectClaimReconciliation`（`:67`）：纯函数，合法状态迁移闭包为 Prepared→Claimed→Invoking→(Succeeded|Failed)；dispose/revoke/unobservable 从任意 active（非 terminal）状态可落 `Unknown`，且 `Unknown` 是 terminal。malformed identity / duplicate active claim / 非法迁移 / terminal 后再迁移 → `Unknown`（fail-closed）。

**producer 设计约定**（若 plan 阶段批准实现）：

1. **依赖 per-binding generation 与 `validate_lease`**：producer 在任何 claim 语义落地前，必须先经 `validate_lease` 确认 lease 未 stale；stale lease 不产生 claim 事件（fail-closed）。F 不自行重建 generation 比较逻辑。
2. **真实生产路径**：哪些真实路径会产生 `EffectClaimPrepared/Claimed/Invoking/Terminal` 事件，须在 plan 阶段枚举——候选是 scoped tool dispatch（`src/tools/scoped/dispatch.rs`）与 MCP 面（`src/gateway/mcp_face/mod.rs`，已补 per-call `CallIdentity`）。
3. **事务边界**：claim、budget reservation、owner generation、fencing token 必须在**同一次** `SessionService::emit_batch` 里提交（原子提交；`InProcessActorSessionService` 是唯一生产 override，`src/session/in_process.rs:293`；`SessionEventStore::append_batch` 单事务落盘，`src/session/store.rs:79`）。禁止把 claim 与 budget 拆成两次 batch。
4. **禁止 batch-id set / 自动 replay / exactly-once**：不引入「已提交 batch-id」的 durable 集合做幂等；不实现 automatic replay invocation；不承诺外部 effect exactly-once（那是 Gate I）。幂等由 store `(session_id, seq)` 主键 + reducer fail-closed 保证。
5. **crash 窗口**（每个窗口都要在 design review 枚举，不能靠文档措辞掩盖）：
   - crash after intent（`Prepared` 前）→ 无 claim 记录，下次重来；
   - crash after claim（`Claimed` 后、`Invoking` 前）→ reducer 闭包为 Claimed，非 terminal；
   - crash after invocation（`Invoking` 后、`Terminal` 前）→ 外部 effect 可能已发生，reducer 无 terminal → `Unknown`；
   - crash after outcome（`Terminal` 后）→ terminal 已 durable，重放不得再次调用。
6. **VerifyOnly 路径**：`ReplayPermit::prepare`（`src/session/replay.rs:43`）在 `effective_input: None`（生产侧无条件）时返回 `ReplayPrepare::Refused`，落到 VerifyOnly/fail-closed；`REPLAY_MAX_ATTEMPTS_PER_CALL = 3`（`src/session/replay.rs:77`）是跨崩溃 cursor 预算上限，耗尽 ⇒ VerifyOnly。这些保持不变。
7. **职责分离**：`ReplayPermit`（bridge 侧 single-use invoke）与 `EffectClaim` reducer（纯 reconciliation）保持分离；reducer 不 invoke，permit 不 reconcile。
8. **外部 effect 不可观测 → Unknown**：外部 effect 无 ledger 时，`Terminal` 缺失 → `Unknown`，且不得推断成功（对应 §5「Unknown 是 terminal」）。
9. **不混计数**：同一 effect 的 request、claim、outcome 计数不得混成 exactly-once——exactly-once 是 Gate I 目标，F 只负责「每个状态迁移 fail-closed 闭包」，不承诺外部幂等。

**门禁**：只有 effective input 已在正确 guardrail/cache/dispatch 边界 durable 记录，且用户单独批准 Safe Replay gate（I）后，才允许实现真实 replay invocation。F 不越过该门禁。

### 3.4 Gate G — committed projection/subscription wiring

**目标**：把纯 `CapabilityChangeStream` 接到真实 registry change feed。

**现有事实**：
- `ToolHandlerRegistry` 已有 committed change feed —— `RegistryShared.change_tx: broadcast::Sender<RegistryChange>`（`src/tools/registry.rs:108`），`mutation_lock`（`src/tools/registry.rs:123`）序列化 ArcSwap 与 change 广播，保证订阅者按 revision 顺序观察事件；`subscribe()` 返回 `broadcast::Receiver<RegistryChange>`（`src/tools/registry.rs:541`）。
- `broadcast::channel(256)`（`src/tools/registry.rs:251`）：**256 槽环形缓冲**，lag 时最旧条目被覆盖丢弃（`broadcast::RecvError::Lagged`）。

**设计约定**：

1. **`ToolHandlerRegistry::subscribe` 是 live broadcast，不是 durable recovery source**：它是有限 256 槽的 live 通知，lag 可丢；**不得声称**它可以断线恢复或作为 recovery 依据。recovery 只认 `SessionEventStore`。
2. **分层：host/adapter 做 I/O，`CapabilityChangeStream` 只做纯 value 状态机**：`CapabilityChangeStream`（`src/capability/facade.rs:94`）**不含** receiver / history / wiring——它只对 `(cursor, changes)` 做严格单调 cursor（`append` 对 `cursor <= committed_cursor` 拒绝）、`(id, revision)` 去重、`invalidate_and_resync` snapshot+delta 重同步。读 registry 的 live broadcast、构造 `CapabilitySnapshot` 与可用 delta 是**具体 host/adapter 的职责**，在同一同步边界完成后把 delta 交给 `CapabilityChangeStream::from_snapshot / append / invalidate_and_resync`。`broadcast::Receiver` 只存在于 host/adapter 内部，不进入纯 value 机。
3. **registry `name: String` + raw `u64` revision 做明确映射**：`RegistryChange { Registered/Replaced/Unregistered { name: String, revision: u64, source: ToolSource } }`（`src/tools/registry.rs`）映射到 `CapabilityChange { Registered/Replaced/Unregistered { id: CapabilityId, revision: CapabilityRevision } }`（`src/capability/facade.rs:62`）：
   - `name: String` → `CapabilityId { namespace: TOOL_NAMESPACE, name }`；
   - raw `u64` revision → `CapabilityRevision(u64)`；
   - `source: ToolSource` 不进入 `CapabilityChange`（丢弃或仅日志）。
4. **atomic snapshot + cursor capture（host/adapter 职责，非 `Zahir::subscribe` 签名）**：具体 host/adapter 在同一 `mutation_lock` 同步边界内先取 `descriptor_snapshot()`（`src/tools/registry.rs:487`）+ `revision()`（`src/tools/registry.rs:501`）构造 `CapabilitySnapshot`，再建 `broadcast::Receiver`；用 snapshot 的 `committed_cursor` 裁剪 `RegistryChange` 得到 delta，交给 `CapabilityChangeStream`。`Zahir::subscribe(scope, cursor) -> CapabilityChangeStream` 的签名不变；`broadcast::Receiver` 不暴露进 `CapabilityChangeStream`。
5. **registry mutation 与 committed event 的顺序**：依赖现有 `mutation_lock` 保证；不得绕过。若 future plan 引入跨 store 的 committed event（非本 registry 的 in-process broadcast），须重新评估顺序保证——本 spec 明确 **registry 的 in-process broadcast 不是 recovery source**。
6. **owner revoke / generation bump 的 Invalidated 语义**：`OwnershipTree::bump/revoke` 触发 `CapabilityChange::Invalidated`，保持 cursor 不回零。
7. **snapshot+delta resync 去重**：复用 `CapabilityChangeStream::invalidate_and_resync` 的 `(id, revision)` 去重。
8. **overflow/lag 语义**：broadcast 落后（`RecvError::Lagged`）时，host/adapter 重新取 snapshot + 新 cursor 构造 delta，走 `invalidate_and_resync`（`Invalidated` 语义），不得伪装成完整逐条投递；纯 value 机不感知 receiver 是否 lag。
9. **`SessionEventStore` 是 durable recovery authority；registry broadcast 无 durable cursor**：断线重连、进程重启后的 committed cursor 恢复只从 `SessionEventStore`（`load_events_range`）读，不依赖 broadcast。registry 的 in-process broadcast **没有 durable cursor**，因此不得声称「从断线恢复」——它只能在同一进程内的 lag 场景做 snapshot + `Invalidated` 重同步。
10. **GlobalBus 仍只做低延迟通知**：不把 GlobalBus 当 change feed。
11. **projection 不成为 recovery source**：StateDatabase / ACP JSON / MCP projections 不能授权、不能 resolve handler、不能决定 replay、不能维护第二份 identity/revision。为每个 projection（ACP/MCP/model）分别证明这四点。

---

## 4. 数据流与 ownership

### 4.1 层级

```
Runtime → Session → Run → Task → EffectClaim
```

- `OwnershipTree`（`src/capability/ownership.rs:216`）已实现 register/resolve/claim/claim_state/bump/revoke/dispose；`LifetimeScope` rank：`External < Task < Run < Session < Runtime`；`bump(scope)` 重写 at-or-below scope 的 binding generation 并清空其 claims；`revoke`/`dispose` 不可逆。本期新增只读 `generation(&CapabilityId, &VisibilityScope) -> Option<OwnerGeneration>`（§3.2 E5）。
- `VisibilityScope`（`src/capability/ownership.rs:80`）是正交 ACL 轴（principal/workspace/session/allowed_kinds/allowed_ids/approval），与 `LifetimeScope` 独立；binding key 为 `(CapabilityId, VisibilityScope)`。
- `EffectClaim`（`src/capability/ownership.rs:118`）是「一个 request 对一个 fence 的 ownership 断言」，`FencingToken`（`src/capability/ownership.rs:101`）只有 `FencingToken::new(nonce)` 一个构造函数（owner 层专属）。

### 4.2 跨 crate wire schema 单源派生

- `SessionEvent::EffectClaim*` 的 wire 身份是 `(request_id, owner, owner_generation, fence)`，由 `src/session/events.rs:710-744` 单一定义；`src/capability/effect_claim.rs::EffectClaimEvent` 是它的 reducer 投影，不是第二份契约——它从 `SessionEvent` 派生（`effect_claim_event_from_session`），不重复定义身份字段来源。
- 判据 #10（跨 crate wire 契约两边各持一份形状互相抵消）：schema fingerprint 的 canonical form 由 `to_descriptor` 单一 source 派生（§3.2 E9），descriptor 与 projection 不得各持一份。
- 判据 #12（顺序/单位/边界同一处派生）：cursor/revision 语义统一派生自 `ToolHandlerRegistry::revision()` 与 `SessionEventStore` seq，不得另起计数器；owner generation 语义统一派生自 `OwnershipTree::generation()`。

---

## 5. 错误语义、幂等边界、并发/顺序、fail-closed

### 5.1 错误语义

- `ResolveError` 闭集（`Unknown / NotVisible / Revoked / StaleOwner`），resolve-time 与 validate-time 矩阵见 §3.2 E3。任何未归类失败都归 `Unknown`（fail-closed）。
- `reconcile_effect_claim` 对任何非法输入返回 `EffectClaimState::Unknown`（fail-closed），不返回「成功」。
- `OwnershipTree::claim` 在 revoked / disposed / owner 不匹配时返回 `None`；`claim_state` 对不存在/被 bump 的 claim 返回 `ClaimState::Unknown`；`generation()` 对不存在的 binding 返回 `None`。
- `SessionService::emit_batch` 默认返回 `Err`（`"this SessionService cannot append atomically"`），只有 `InProcessActorSessionService` 提供原子 override——这本身就是「不能原子提交就说不能，而非报一个会撕裂的成功」的 fail-closed 形状。
- `SessionEventStore::append_batch` 空 events 且无 retire 被拒绝（「报成功的 no-op」判据 #11 的防御）。

### 5.2 幂等边界

- `CapabilitySlot::install` 幂等（第二次调用返回 `false`）；`RegistrationHandle::dispose` 幂等（generation-guarded，重复调用 `false`）。
- `SessionEventStore` 的 `(session_id, seq)` 主键保证重复 append 冲突失败而非重复落盘。
- effect claim 的幂等边界在 F：reducer 对 duplicate active claim / terminal 后再迁移 → `Unknown`，**不**做「第二次调用等于第一次」的语义承诺——外部 effect 幂等属 Gate I。
- 不引入 batch-id durable set / duplicate-ignore（§1.3 非目标）。

### 5.3 并发 / 顺序

- `ToolHandlerRegistry` 的 `mutation_lock` 序列化「ArcSwap 状态交换 + change 广播」，保证 revision 与 change event 同序发布（`src/tools/registry.rs:111-119` 注释明确这是防 `swap(rev=2); swap(rev=1); send(rev=1); send(rev=2)` 交错）。
- `CapabilityChangeStream` 只接受严格单调 cursor（`append` 对 `cursor <= committed_cursor` 拒绝），`invalidate_and_resync` 对 `snapshot.committed_cursor < old` fail-closed 不突变。
- broadcast 是 256 槽环形缓冲，lag 丢弃最旧条目（`RecvError::Lagged`），由 `invalidate_and_resync` 以 snapshot+新 cursor 重同步，不伪装逐条投递。
- `SessionService::emit_batch` 是唯一原子多事件提交路径；并发 writer 靠 store 事务与 seq 主键串行化。
- `OwnershipTree::generation()` 是只读，与 `bump/revoke/dispose` 的写共享同一 `Mutex<OwnershipInner>`，读到的 generation 与最近一次写入一致。

### 5.4 fail-closed 清单

以下场景全部 fail-closed，不得「报成功」：

| 场景 | fail-closed 行为 |
| --- | --- |
| lookup 未命中 / binding 缺失 | `resolve` → `Err(ResolveError::Unknown)` |
| 调用方 `Reference.visibility` 无对应 binding | `resolve` → `Err(ResolveError::NotVisible)` |
| binding 已 revoke | `resolve`/`validate_lease` → `Err(ResolveError::Revoked)` |
| stale owner generation（use-time） | `validate_lease` → `Err(ResolveError::StaleOwner)`，不返回 handler |
| 旧 lease 的 descriptor 与当前 registry descriptor 不一致（replace/unregister 后） | `validate_lease` → `Err(ResolveError::Unknown)` |
| malformed claim identity（空 request_id/owner、0 generation、0 fence） | reducer → `Unknown`（`src/capability/effect_claim.rs` 已测） |
| duplicate active claim | reducer → `Unknown` |
| terminal 后再次迁移 | reducer → `Unknown` |
| schema mismatch | registry `register/replace` 校验 `matches_definition` 失败 → 拒绝 |
| 外部 effect 不可观测 | F：`Terminal` 缺失 → `Unknown` |
| snapshot 落后于 subscriber cursor | `invalidate_and_resync` 返回 `false` 不突变 |
| broadcast lag | 不伪装逐条投递，snapshot+新 cursor 重同步 |
| emit_batch 无法原子提交 | `Err`，不假装成功 |
| 空 batch 空 retire | `append_batch` → `Err`，不报成功 no-op |

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

1. **resolve error 闭集**：lookup 未命中 → `Unknown`；visibility 不匹配 → `NotVisible`；revoke 后 → `Revoked`；resolve 成功 → `Ok(lease)` 且 `lease.owner_generation == OwnershipTree::generation()`（证明 resolve 铸造的是 fresh lease，不产生 `StaleOwner`）。
2. **visibility 分离**：同一 `CapabilityId` 在两个不同 `VisibilityScope` 下注册，`Reference { id, visibility: A }` 与 `visibility: B` 各自独立 resolve（对应 `ownership.rs` 的 `same_id_two_scopes_coexist`）。
3. **generation read/bump/revoke/dispose**：`OwnershipTree::generation()` 返回当前 per-binding generation；`bump(scope)` 后读到新值；`revoke(capability)` 后返回 `None`；`dispose(scope)` 后 at-or-below scope 的 binding 返回 `None`。
4. **validate stale lease**：resolve 得 lease → `bump` → `validate_lease(&lease)` → `Err(StaleOwner)`；未 bump 时 → `Ok(())`；revoke 后 → `Err(Revoked)`；`replace` 更新 descriptor（revision/fingerprint 变）后 → `Err(Unknown)`（旧 lease 不得作为当前凭证）。
5. **fingerprint 稳定性**：同一 schema 两次 `to_descriptor` fingerprint 一致；不同 schema fingerprint 不同；describe/project 复用该 fingerprint（不重算）。
6. **projection 与 dispatch 同代**：projection 不旁路 registry（复用 describe 的 snapshot）。
7. **registry mapping**：`RegistryChange { name, revision: u64 }` → `CapabilityChange { id: CapabilityId{namespace: TOOL_NAMESPACE, name}, revision: CapabilityRevision(u64) }`。
8. **lag resync**：落后 broadcast 订阅者收到 snapshot + 新 cursor，`invalidate_and_resync` 去重 `(id, revision)` 且 cursor 不回零。

**F 新增测试**（若实现 producer）：

1. EffectClaim replay/recovery：从 `SessionEventStore` 读回 claim 事件，`reconcile_session_events` 与真实 event 一致。
2. 每个 crash 窗口（intent/claim/invocation/outcome）的 reducer 终端态断言。
3. 外部 effect 不可观测 → `Unknown`（不推断成功）。
4. **reducer fail-closed**：duplicate active claim / 非法迁移 / terminal 后再迁移 → `Unknown`。
5. **atomic emit boundary**：claim+budget 同一 `emit_batch` 提交；「报成功的 no-op」负例：空 events 空 retire 被拒。

**G 新增测试**：

1. projection ordering：mutation 顺序与 change event 顺序一致（依赖 `mutation_lock`）。
2. reconnect/overflow：落后订阅者收到 snapshot+新 cursor，不伪装逐条投递。
3. **projection-only / notification-only 四点证明**：ACP/MCP/model projection 各证明（不能授权 / 不能 resolve handler / 不能决定 replay / 不维护第二份 identity/revision）；GlobalBus 只做通知不持久化——以 conformance fixture 形式记录，**不把 H 的完整 ACP server 当作 E–G 验收项**。

**禁止**：`cargo fmt --check` 全仓运行会被既有无关文件格式漂移阻断；改动文件须单独核验格式，报告区分 baseline drift 与本次变更。

---

## 7. 旧代码清理与文档更新清单

| 项 | 动作 | 说明 |
| --- | --- | --- |
| `src/capability/backend.rs:103` 的 `fingerprint: [0u8; 32]` 占位 | 由 E 在 `to_descriptor`（唯一 owner）用 sha2 替换为 canonical schema digest | 关闭「未计算 fingerprint」gap（§3.2 E9） |
| `src/capability/backend.rs:25,73` 的 `CapabilityBackend::generation()` 与常量 `OwnerGeneration(0)` | 删除设计承诺；owner generation 改经 `OwnershipTree::generation()` 读取 | 关闭占位，避免 descriptor 自构造即 stale（§3.2 E6） |
| `src/capability/descriptor.rs` 的 `CapabilityDescriptor.owner_generation` | 删除设计承诺；不参与任何 stale/授权判定 | generation 权威只在 lease + `OwnershipTree`（§3.2 E6） |
| `src/capability/facade.rs:222-226` 的 `Zahir` trait | `resolve` 改 `Result<BackendLease, ResolveError>`，新增 `validate_lease`；`Reference`/`Scope` 加 `visibility` | §3.2 E1/E2/E7 |
| `src/capability/ownership.rs` 的 `OwnershipTree` | 新增只读 `generation()` | §3.2 E5 |
| `docs/reference/FEATURE_LOCATOR.md` §3.5d | 更新为「E/F/G 已落地」状态（或「部分落地 + deferred」） | 明确哪些 deferred 仍成立 |
| `docs/reference/ARCHITECTURE.md` / `DESIGN_PATTERNS.md`（如涉及） | 补 authority/projection/notification 三类边界 | 与 §3.1 对齐 |
| `docs/superpowers/specs/2026-10-06-capability-phase4-design.md` §12 非本期范围 | 交叉引用本 spec | 保持上下游一致 |
| `AGENTS.md` 的「Capability Phase 4 入口」段 | 同步 E/F/G 结论（改 `main` 前先核验） | 与 `CLAUDE.md` 同步 |
| 空引用 `deepseek-harness/packages/capability/` | 从 gap analysis / 文档中剔除 | 实际不存在（§2.1） |
| 无第二套 registry/ownership 残留 | review diff 检查 | 判据 #16 孪生子系统防御 |

---

## 8. 实施顺序与双线并行依赖

### 8.1 顺序

1. **Gate E**（先）——它是 F/G 的前置：F 的 producer 需要 E 的 `validate_lease` + per-binding generation，G 的 committed feed 需要 E 的 `subscribe` 落地。
2. **Gate F**（design review → 决定是否实现 producer）——依赖 E 的 ownership/lease 语义；不阻塞 G。
3. **Gate G**（可与 F 双线并行）——依赖 E 的 `subscribe` + registry change feed，与 F 无直接数据依赖。

### 8.2 双线并行

- 线 1：E → F（recovery/producer 线）。
- 线 2：E → G（projection/subscription 线）。
- E 完成后 F 与 G 可并行；二者共享「`ToolHandlerRegistry` change feed」「`OwnershipTree`」只读依赖，无写入冲突，但须避免同一文件（`facade.rs` / `backend.rs` / `ownership.rs`）的编辑竞争——plan 阶段应把共享文件改动（`Zahir` trait、`ResolveError`、`OwnershipTree::generation()`、`Reference`/`Scope` 加 `visibility`、`to_descriptor` fingerprint）归入 E，F/G 只新增各自模块或测试。

### 8.3 低内存时 cargo 启动前检查

- 低内存环境跑 `cargo check/test` 前，先确认 `src/capability/` 与 `src/session/` 相关 crate 目标范围（`-p alephcore`），避免全 workspace 构建触发高内存。
- 启动前先 `git status --short --branch` 确认 worktree 干净、HEAD 与上游基线一致，再 `cd` 到对应 worktree 显式执行；不依赖 shell 默认 cwd。
- 建议按 gate 拆分 `cargo test -p alephcore --lib <mod>` 定向运行，而非全量，减少峰值内存与编译时间。

---

## 9. 红线对齐（R1 / R4 / R6 / R7 / R8 / R9 / R10）

| 红线 | 本 spec 的对齐方式 |
| --- | --- |
| **R1 大脑与四肢绝对分离** | capability facade / ownership / effect-claim 全部在 Rust Core（`src/capability/`、`src/session/`、`src/tools/`），不调用任何平台 API，不经 Bridge IPC 之外与四肢交互。 |
| **R4 Interface 层禁止业务逻辑** | Channel/Bot/CLI/Panel 纯 I/O；`Zahir` / `OwnershipTree` / `EffectClaim` reducer 是 Core 业务，不落入 interface 层。 |
| **R6 一核多端** | `Zahir` 是 Core 内唯一读面；多端只通过 projection 读、通过 `resolve`/`validate_lease` 走授权，不各自维护 capability 事实。 |
| **R7 LLM 主权** | 本 spec 只钉「纯 value 状态机 + 授权/lease 契约 + fail-closed 闭包」，不引入任何确定性代码替代 LLM 推理。 |
| **R8 工具即一切** | capability 是工具的可配置元层（describe/resolve/subscribe/project），不新增旁路；所有可配置操作仍暴露为工具。 |
| **R9 智慧在 Prompt 中** | 不把中间件智慧迁移进代码；`ResolveError`/`validate_lease`/reducer 只承载契约与 fail-closed，不承载推理策略。 |
| **R10 薄 Harness** | `src/harness/` 不扩张；本 spec 的所有新增类型/方法落在 `src/capability/`，harness 12 文件棘轮不受影响。 |

---

## 10. 风险与迁移影响

**风险**：

1. **`resolve` 返回类型是 breaking trait change**：`Zahir::resolve` 从按值 `BackendLease` 改为 `Result<BackendLease, ResolveError>`，任何未来 impl 与现有消费点都要迁移。当前 `Zahir` 无 concrete impl（仅 trait 定义 + `CapabilityChangeStream` 纯值测试），爆炸半径 = trait 定义 + 未来 impl + 相关测试；仍有「消费方误假设 resolve 永不失败」的回归风险，靠 E 新增测试覆盖。
2. **`Reference`/`Scope` 加 `visibility` 是 struct shape change**：构造 `Reference`/`Scope` 的调用方必须显式提供 `visibility`（或显式 `..Default::default()`），否则编译失败；这是有意的编译期强制，但会带来一次性迁移成本。
3. **`OwnershipTree` 生产未接线**：当前生产代码未构造 `OwnershipTree`（grep 仅 `src/capability/ownership.rs` 自身 + 测试 + `backend.rs` 注释引用）。E 必须新增 wiring seam（`ConcreteZahir` 初始化时向 `Arc<OwnershipTree>` 注册 runtime/default binding）。这是 E 的实质工作量，不只是「换个返回类型」。
4. **fingerprint 计算引入 sha2 成本**：每个 descriptor fold 一次 `Sha256`，成本受 descriptor 数量约束，digest 缓存在 `SchemaRef`（`Copy`）中不重复计算。canonical 序列化必须稳定（键序固定），否则 fingerprint 不稳定。
5. **broadcast 256 槽 overflow 是常态**：lag 时丢事件不是 bug 而是设计约束；任何把 broadcast 当「可靠逐条投递」的假设都会在 overflow 下错。G 的测试必须显式覆盖 `RecvError::Lagged` → resync 路径。

**迁移影响**：

- `CapabilityBackend` trait 移除 `generation()`：只有 `ToolBackendAdapter` 实现，删除即完成；`CapabilityBackend` 的其它 impl（若有未来 backend）无需迁移。
- `CapabilityDescriptor.owner_generation` 删除设计承诺：若实现保留字段，须标注「非权威、不参与判定」；若移除字段，需同步 `to_descriptor` 与任何反序列化/投影代码。
- 新增 `ResolveError`、`validate_lease`、`OwnershipTree::generation()` 是增量（additive），不破坏既有调用点。
- `to_metadata_form`（`src/tools/service.rs:453`）继续只做旧 Tool metadata compatibility wrapper，不改为 canonical identity 生成器。

---

## 11. 附录：关键源码锚点

- `src/capability/descriptor.rs` — `CapabilityKind`（9 种）、`CapabilityId`、`CapabilityRevision(u64)`、`SchemaRef { version: u32, fingerprint: [u8; 32] }`、`CapabilityDescriptor`（含 `owner_generation` 字段，其设计承诺本期删除）。
- `src/capability/facade.rs` — `Scope`（:18）、`Reference`（:27）、`Cursor`（:36）、`CapabilitySnapshot`（:44）、`BackendLease`（:55，仅 `descriptor + owner_generation`）、`CapabilityChange`（:62）、`CapabilityChangeStream`（:94）、`TransportTarget`（:201）、`Projection`（:209）、`Zahir` trait（:222-226，本期改 `resolve` 返回类型并新增 `validate_lease`）。新增 `ResolveError`。
- `src/capability/backend.rs` — `TOOL_NAMESPACE = "aleph/tools"`、`CapabilityBackend` trait（:25，本期删除 `generation()` 承诺）、`ToolBackendAdapter`（:33）、`impl CapabilityBackend`（:54）、`generation()` 常量 `OwnerGeneration(0)`（:73，本期删除）、`to_descriptor`（:92，fingerprint 唯一 owner，:103 当前 `[0u8; 32]` 占位）。
- `src/capability/ownership.rs` — `OwnerGeneration(u64)`（:23）、`LifetimeScope`、`VisibilityScope`（:80）、`OwnerRef`、`FencingToken`（:101，唯一构造 `new(nonce)`）、`EffectClaim`（:118）、`ClaimState`（:177）、`RegisterError`（:184）、`OwnershipTree`（:216，`key()` :233，`new` :222，`register` :244，`resolve` :272，`claim` :278，`claim_state` :306，`bump` :339，`revoke` :352，`dispose` :360）。新增只读 `generation(&CapabilityId, &VisibilityScope) -> Option<OwnerGeneration>`。
- `src/capability/effect_claim.rs` — `EffectClaimState`（:6）、`EffectClaimEvent`（:9）、`EffectClaimReconciliation`（:18）、`effect_claim_event_from_session`（:31）、`reconcile_session_events`（:62）、`reconcile_effect_claim`（:67）。reducer 在此文件，**不存在 `src/session/reducer.rs`**。
- `src/capability/mod.rs` — `CapabilitySlot`、`MissingSemantics`、`Outcome`。**不存在 `src/capability/registry.rs`**（registry 是 `src/tools/registry.rs`）。
- `src/session/events.rs:710-744` — `SessionEvent::EffectClaimPrepared/Claimed/Invoking/Terminal`、`ApprovalMemo`、`HookMemo`（`Durability::Normal`）。
- `src/session/store.rs` — `SessionEventStore` trait（`append_batch` :79 单事务）、`SqliteEventStore`、`(session_id, seq)` 主键、空 batch 无 retire 拒绝（:1231）。
- `src/session/service.rs:78` — `SessionService::emit_batch` 默认拒绝、`InProcessActorSessionService` 唯一生产 override（`src/session/in_process.rs:293`）。
- `src/session/replay.rs` — `ReplayPermit`（:51，`pub(crate)`、single-use）、`prepare`（:43）、`ReplayPrepare::Ready/Refused`、`REPLAY_MAX_ATTEMPTS_PER_CALL = 3`（:77）。
- `src/tools/registry.rs` — `ToolHandlerRegistry`、`RegistryChange`（name: String + raw u64 revision + source）、`RegistryShared { inner: ArcSwap, change_tx: broadcast::Sender, mutation_lock }`（:108/:123）、`broadcast::channel(256)`（:251）、`register/replace/unregister/descriptor`（:409）/`descriptor_snapshot`（:487）/`revision`（:501，raw u64）/`subscribe`（:541）。
- `src/tools/service.rs:453` — `to_metadata_form(defs: &[ToolDefinition]) -> Arc<[ToolDefinition]>`（compatibility wrapper）。
- `Cargo.toml:113,261` — `sha2 = "0.10"`（fingerprint 复用，不新增依赖）。
