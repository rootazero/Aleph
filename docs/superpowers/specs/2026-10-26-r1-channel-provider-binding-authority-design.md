# R1 Channel Provider Binding Authority — 设计提案 / Design proposal

日期 / Date: 2026-10-26

**状态：Design only / Awaiting user approval。** 用户已选择方案 1 的方向；本文细化该方向，但仍需用户审查批准才能实施。**R1 保持 NO-GO / deferred；不宣称 R1 GREEN。** 本次仅更新本文档，不修改 Rust、既有 provider-source-lease spec、测试、计划或其他文件。

本文中的类型、签名、锁协议、资源 receipts 和测试均为 **PROPOSED / 尚未实现**，除明确标为 current evidence 的源码事实外，不应读成当前行为。它不是 implementation plan，也不授权 R2 Feishu、backend mount 或外部消息发送。

## 1. Intent / 问题与意图

需要证明的不是“capture 有两个 provider 字段”，而是：**被调用的 channel、管理它实际 start/stop 的 provider、发布的 descriptor，以及被接纳的工作，来自同一个真实 source owner 的 canonical entry。**

当前 `ChannelRegistry` 已经拥有 channel 实例、desired intent、显式生命周期和 webhook mount；因此扩展这个现有 runtime owner，比新增一张 provider/channel 映射更直接。代价是该 owner 必须承担 publication、admission、drain 和 completion acceptance 的同步责任；旧通道不能靠自动生成的 Ready provider 获得新 Capability 权限。

**Builder-check decision / 用户委托的实现决策：**用户将“canonical descriptor/schema 描述生命周期还是具体 send”的选择委托给最佳实践判断；本次明确裁定：**此 R1 slice 只描述 source-owned channel/provider lifecycle declaration，不发布 concrete send 的 Callable capability。** 下文 send 是经 source 接纳的内部工作及执行/排空证据，不是生命周期 descriptor 的可调用操作。未来公开 send 必须有独立 operation identity 和真实 operation schema，不能夹带进生命周期声明。该问题已解决，不再作为开放选项；此裁定不等于用户批准整份设计或授权实施，R1 仍须实现、验证并另行 gate ruling。

### 1.1 Current evidence / 当前源码锚点

以下行号取自本步骤开始时的**当前工作区**，包含用户已有未提交修改，不是仅引用 HEAD，也不继承旧规格的历史行号。后续实现会使行号移动。

| 事实 / Fact | 当前源码证据 / Exact source anchor |
|---|---|
| `ChannelHandle` 是 `Arc<RwLock<Box<dyn Channel>>>`；`ChannelCapture` 只有 `channel_id`、`source_revision`、`handle` | `src/gateway/channel_registry.rs:42-59` |
| registry 持有 channels、factories、webhook mounts、`lifecycle_sync`、source revision、desired state；同步锁不是另一份实例 authority | `src/gateway/channel_registry.rs:212-245` |
| source capture 在同一 lifecycle/read boundary 取得 handle 和 revision；validation 只比较当前 channel `Arc` | `src/gateway/channel_registry.rs:542-575` |
| register/create replacement 先 stop outgoing；stop 失败保留 incumbent；legacy register 只返回 ID，不能表达完整失败 | `src/gateway/channel_registry.rs:324-388`、`src/gateway/channel_registry.rs:415-482` |
| unregister 先 stop/unmount，再移除；`Arc::try_unwrap` 失败时 `None` 不等于“没有移除” | `src/gateway/channel_registry.rs:484-540` |
| 当前 revision 在 map mutation 后增长，耗尽时 `expect`；没有可恢复的预留 publication revision 事务 | `src/gateway/channel_registry.rs:578-588` |
| start/stop 通过实际 channel，成功 start 后启动 forwarder / mount；stop 先改 desired state，再调用实际 cleanup | `src/gateway/channel_registry.rs:678-743` |
| forwarder 单独持有 channel Arc，不能以 registry 删除或 Arc drop 冒充 task join | `src/gateway/channel_registry.rs:488-501`、`src/gateway/channel_registry.rs:1355-1368` |
| Core 的 Channel 是 slot API 的 re-export | `src/gateway/channel.rs:1-4` |
| generic Channel 提供 info/state/start/stop/send，没有 ProviderHandle binding，也未提供 canonical operation JSON schema | `crates/aleph-slot-api/src/channel.rs:865-875`、`crates/aleph-slot-api/src/channel.rs:960-967` |
| generic factory 只创建 `Box<dyn Channel>`，`create_with_id` 默认转调 create；没有 provider 产物 | `crates/aleph-slot-api/src/channel.rs:1157-1181` |
| in-tree config 构造入口不只是 registry factory 路径，boot 也直接 register_for_boot | `src/gateway/handlers/channel.rs:469`、`src/bin/aleph-server/commands/start/builder/subsystems.rs:567` |
| ProviderHandle 持生命周期 Arc，`same_instance` 比实际 Arc；不是比较 ProviderId | `src/capability/provider.rs:234-257` |
| generation 在进入 Starting 时 checked increment；`acquire_call` 要求 Ready 且 generation 相等 | `src/capability/provider.rs:160-201`、`src/capability/provider.rs:329-334` |
| acquire_call 只是 admission 检查，不 drain、不取消、不保证安全卸载 | `src/capability/provider.rs:323-328` |
| provider 已有 Ready commit 的同步 callback；锁序 provider → registry mutation，不能在锁内执行外部代码 | `src/capability/provider.rs:259-305` |
| OwnershipTree 以 capability + visibility 查独立 owner generation；bump/revoke/dispose 不由 provider 推导 | `src/capability/ownership.rs:394-409`、`src/capability/ownership.rs:511-600` |
| kind 为九类，facets 是行为描述；descriptor 不携带 provider/owner generation | `src/capability/descriptor.rs:23-50`、`src/capability/descriptor.rs:149-161` |
| Tool snapshot 从 canonical publication 派生；schema hash 只覆盖 canonical input schema | `src/capability/backend.rs:65-87`、`src/capability/backend.rs:119-148` |
| Tool ReadySnapshot 包含 plugin/WASM/handler/registry-specific tuple；完整 descriptor hash 与 schema hash 不同 | `src/tools/registry.rs:45-97` |
| Tool canonical lease 对未变 entry 不因无关 publication 失效；自身不是 channel lease | `src/tools/registry.rs:149-157`、`src/tools/registry.rs:208-249` |
| Tool handler 在 await 前后验证 producer proof；legacy 无 Ready proof 不具备相同 fence | `src/tools/registry.rs:316-371` |
| 当前 BackendLease 直接持 Tool CanonicalEntryLease；Zahir 没有 invoke 方法 | `src/capability/facade.rs:63-74`、`src/capability/facade.rs:271-277` |

### 1.2 对先前 RED 探针的纠正 / Correcting the probe

`src/gateway/channel_registry.rs:3368-3388` 的 `capture_exposes_provider_instance_and_generation_for_admission` 独立创建了一个 `ProviderHandle::new(ProviderLifecycle::ready(...))`，随后只向 registry 注册 `WebhookyChannel`。**这个 provider 没有进入 channel 的构造、registration 或 lifecycle。**

缺字段导致的编译 RED 只能指出 API 缺口，不能证明测试期望的 provider 就是 channel owner。旧规格中的 RED 记录不作为本设计已闭合的证据；本文不修改那份记录或探针。

**明确拒绝：**只给 `ChannelCapture` 添加 `provider_handle` / `provider_generation`，从测试局部变量、字符串 ID、独立 map 或消费者参数补齐。即使这样能编译或满足数值断言，也没有 source provenance，不能准入。

## 2. Goals / Non-goals / Anti-goals

### Goals / 目标

1. 保留一个 `ChannelRegistry` 实例 authority，在原 canonical registration entry 内原子发布 channel 与 source-minted provider binding。
2. capture 同时绑定 source 实例、entry identity/revision、生命周期声明 descriptor/schema fingerprint、实际 ProviderHandle 与 generation；schema 不描述 send 输入/输出。
3. 在实际 use boundary 接纳工作；在独立 completion boundary 拒绝 stale result；stop 能证明 drain 和资源清理。
4. provider generation 与 OwnershipTree owner generation 各回各自 authority 验证，互不替代。
5. 所有通道共用契约；非 Feishu fixture 首先验证，不能由 Feishu 协议决定 Core 形状。
6. 通过真实分配/调用边界 receipts，证明 disabled/discovered/cold read 不初始化 provider runtime。

### Non-goals / 非目标

- 本步骤不实现任何 API、测试或 provider，不改既有 spec，不做生产挂载。
- 不发布 send Callable capability，不设计 send operation JSON schema，不将 lifecycle schema 当作 send 参数/返回值契约；内部 send 准入/完成证据不扩大公开能力面。
- 不重新设计 Tool lease，不建 universal erased lease/envelope，不迁移全部 Gateway callers。
- 不实现 durable effect admission、EffectClaim producer、Safe Replay、external exactly-once、跨进程 fencing 或 crash recovery。
- 不把 provider broadcast / source revision 当 session durable event log；不改 SessionEventStore。

### Anti-goals / 明确禁止

- 新建 `ChannelProviderRuntime`、parallel registry、Core validator dispatch table、channel → provider/owner/disposer 第二份 map。
- 让 ProviderHandle 拥有 `Box<dyn Channel>`，或让纯 slot API 依赖 Core 的 ProviderHandle。
- 在 get/list/describe/project/resolve 内“顺便”创建 Ready provider、注册 handler、认证或启动 runtime。
- 从 ChannelStatus、enabled、channel type、相同 ProviderId 或相同数字 generation 推断 authority。
- 复制 Tool 的 ReadySnapshot / CanonicalEntryLease / WASM tuple，或把 channel 伪装为 Tool 来绕开缺失的 source binding。
- 因为有 `Channel::send` Rust 方法就臆造 canonical operation JSON schema，或给生命周期 descriptor 偷加 Callable/send operation 语义。

## 3. Constraints / 红线与设计边界

- 遵循 `docs/reference/REDLINES.md:13-22`：Core 不调用平台 API，Channel 保持 I/O，确定性租约校验不承担 LLM 推理，不增重依赖、不进入 Harness。本文 R1 gate 名称与宪法 R1 平台隔离红线不是同一编号体系。
- Everything externally composable is a Capability；但契约提案不等于 runtime 已接线。维持九类 kind；不增加 `Channel` 第十类。
- descriptor 是投影，不是 provider/owner 权威；MCP、CLI、Panel、RPC 只能消费同一 entry 的投影，不再登记一次。
- lifetime 与 visibility 正交；一个 binding 的 revoke/dispose 不自动等于独占 provider stop。当前 lifetime 没有 channel-instance scope（`src/capability/ownership.rs:163-201`），不能临时用 Session 名称冒充它。
- source 的 canonical entry 是唯一 association authority；ProviderHandle 仍拥有 provider state/generation，OwnershipTree 仍拥有 owner generation/tombstones。
- 所有新增闭包型同步边界仅允许内部、无 await、无 I/O 的小提交，不能成为用户插件回调入口。

## 4. Approaches / 方案比较

| 方案 | 判断与代价 |
|---|---|
| **1. 扩展现有 ChannelRegistry runtime owner（选择）** | 在原 entry 内承载 binding、publication 和 admitted-work accounting。复用实际 start/stop/mount authority；代价是必须明确并升级原生命周期同步协议，旧通道需要显式迁移。 |
| 2. ProviderHandle owns channel（拒绝） | ProviderHandle 当前是轻量 lifecycle handle；让它保存 channel 会逆转已有所有权、引入 Gateway/异步资源耦合，和 registry 的 stop/mount 权威冲突。ProviderHandle 不是 transport runtime。 |
| 3. Separate ChannelProviderRuntime / registry（拒绝） | 两套 instances、enabled、generation、teardown authority 需要对账；相同 ID 也不能证明同一实例。多加 adapter/facade 不会修复 source provenance。 |
| 4. 仅增加 capture 字段（拒绝） | 字段没有可信生产者，unrelated provider 可以被拼入；旧 RED 探针正暴露此问题。 |
| 5. 通用化 Tool lease（延后） | 现有 proof 含插件与 WASM 特有事实，泛化将掩盖 channel 的 admission/drain 问题；第二个真实 callable source 之前不建通用 envelope。 |

**Decision:** ChannelRegistry is the source-owned Gateway provider binding authority, not a new provider implementation. 所谓 source-provided binding，是这个实际 runtime owner 在自己的注册事务中铸造并保管的 binding，不是调用者任意提供一只 ProviderHandle。

## 5. Chosen authority / canonical entry

### 5.1 Minting and publication / 谁来铸造

**PROPOSED：**把现有 `channels` map 的 value 从裸 ChannelHandle 扩展为一个 canonical registration entry。不是另开 map。entry 的 lightweight `Legacy` / `Bound` 形态也在同一 value 内。

- Legacy 注册继续接纳 `Box<dyn Channel>`，**没有 provider binding**；Capability admission 返回 `UnboundLegacy`，不得 synthesize provider。
- Bound 注册只由可信 Gateway composition/lifecycle owner 的显式入口触发。调用者提供 channel 与纯声明 contract，**不接受外部 ProviderHandle 参数**。
- registry 在同一次注册准备中创建 channel Arc、轻量 `ProviderLifecycle::new` handle、private binding 和 immutable publication。该 handle 必须是后续实际 `channel.start/stop` 使用的那个 lifecycle handle。
- 默认只发布 Discovered/Cold/Disabled 声明态；不以 `ProviderLifecycle::ready` 把旧通道“认证”为新 source。只有实际 activation 和 publication 成功才能 Ready。
- source instance identity 使用 registry 内部唯一 Arc token；entry identity 使用每次新 publication 的 Arc 身份。不同 registry、同 ID / 同 revision 的对象不能混用。
- trusted composition 注入**同一** `Arc<OwnershipTree>`；registry 只持引用，不复制 owner map。不得每次 admission 接受任意 caller-supplied tree。显式注册管理操作建立 tree binding；读操作不得借此复活 revoked/disposed binding。

### 5.2 Proposed types / 类型草案（非现有 API，非可直接编译代码）

```rust
// PROPOSED; private fields, no public arbitrary-pair constructor.
enum ChannelRegistrationEntry {
    Legacy(ChannelHandle),
    Bound(Arc<BoundChannelEntry>),
}

struct BoundChannelEntry {
    channel: ChannelHandle,
    binding: GatewayProviderBinding,
    publication: ChannelPublication,
    work: Mutex<ChannelWorkState>,
}

struct GatewayProviderBinding {
    provider: ProviderHandle,
    source_instance: Arc<ChannelSourceIdentity>,
}

struct ChannelPublication {
    descriptor: CapabilityDescriptor,
    lifecycle_schema: serde_json::Value, // Declaration schema, not send input/output.
    descriptor_fingerprint: [u8; 32],
    entry_revision: CapabilityRevision,
}

struct BoundChannelCapture {
    entry: Arc<BoundChannelEntry>,
    source_revision: u64,
    provider_generation: u64,
}

struct ChannelAdmissionLease {
    capture: BoundChannelCapture,
    visibility: VisibilityScope,
    owner_generation: OwnerGeneration,
}

// Non-Clone; owned by the actual executing work, not a caller's cached handle.
struct AdmittedChannelWork {
    lease: ChannelAdmissionLease,
    ticket: ChannelWorkTicket,
}
```

`ChannelWorkState` 只持该 entry 的 in-flight accounting、retiring/draining operation marker 和本地 completion handoff，不维护另一份 provider state/generation/owner generation。`ChannelWorkTicket` 是 source-minted 一次性 identity，不可序列化或由字符串重建；类型细节留给实现审查，不能用消费者填写的 bool 代替。

`ChannelLifecycleContract` 只提供受信任的 capability identity/visibility/lifetime/owner 与 versioned lifecycle declaration schema；不是 operation contract，也不接受 send 参数/返回值 schema。registry 验证 channel ID 一致性及声明范围后计算 publication，调用者不能直接给出“已验证 capture”。声明不携带认证令牌，fingerprint 不散列 secrets。`lifecycle_schema` 的事实源是 source-owned 生命周期声明类型，详见 §8；它不是从 Channel/ChannelFactory 自动提取的操作 schema。

### 5.3 Proposed methods / 签名草案（全部未实现）

```rust
impl ChannelRegistry {
    // Trusted composition supplies the existing tree once, before use.
    pub(crate) fn with_capability_ownership(self, tree: Arc<OwnershipTree>) -> Self;

    // Explicit lifecycle declaration opt-in; mints the provider, not a send capability.
    pub(crate) async fn register_bound_channel(
        &self,
        channel: Box<dyn Channel>,
        contract: ChannelLifecycleContract,
        boot_intent: DesiredChannelState,
    ) -> Result<ChannelId, ChannelBindingError>;

    pub(crate) async fn capture_bound_channel(
        &self, id: &ChannelId,
    ) -> Result<BoundChannelCapture, ChannelBindingError>;

    // Internal source work admission, not public Callable resolution.
    pub(crate) async fn resolve_channel_admission(
        &self, capture: BoundChannelCapture, visibility: &VisibilityScope,
    ) -> Result<ChannelAdmissionLease, ChannelBindingError>;

    pub(crate) async fn admit_channel_work(
        &self, lease: ChannelAdmissionLease,
    ) -> Result<AdmittedChannelWork, ChannelBindingError>;

    // SendResult is internal execution evidence, not a published result schema.
    pub(crate) async fn complete_channel_work(
        &self, work: AdmittedChannelWork, result: ChannelResult<SendResult>,
    ) -> ChannelCompletionDecision;
}
```

`ChannelCompletionDecision` 提议区分 `Accepted(result)` 和 `DiscardedStale(reason)`；transport error 是 Accepted 内的真实结果，不伪装成 stale，也不自动重试。`ChannelBindingError` 至少区分 absent、unbound legacy、wrong source、stale entry/provider/owner、not ready、retiring、revoked/disposed、revision exhaustion、cleanup residual。它是 source-local 错误，不擅自扩充 public ResolveError 闭集。

resolve 只产生待 use-time 再验的 lease，不接纳工作。以上方法是 source-local 内部准入/完成边界，不是公开 send API；`ChannelResult<SendResult>` 仅保留真实 transport 结果供 completion/drain 验证，不定义 descriptor 的 schema。以上 source-local lease **不是**现有 `BackendLease`；不替换其 Tool 字段，不经 Zahir 伪造 invoke。

## 6. Linearization / publication、capture、admission、completion

### 6.1 Synchronization protocol / 提议的同步协议

现有 `lifecycle_sync` 在 start/stop I/O 期间也持有；不能照搬后让 completion 也等待同一锁，否则 stop 等 drain、completion 等 stop 会死锁。**PROPOSED：**在同一 registry 内分阶段：短 gate 提交 intent/retirement，释放 gate 后 drain / I/O，再重取 gate 提交 cleanup/publication。entry 内 operation marker 阻止同一 channel 的第二个 lifecycle operation；不新增全局 owner registry，也不把运行中的 channel I/O 包在 publication mutex 里。

源线性化 gate 持有时先读取当前 map entry，再释放 map guard，但保留 gate；后续同步 callback 不 await。admission/completion 的最终同步锁序：**source lifecycle gate → OwnershipTree binding guard → ProviderHandle lifecycle guard → entry work mutex**。map publication 单独在 source gate 内提交，不与 tree/provider/work 同步 guard 嵌套；Bound entry 发布时尚非 Ready。Ready activation 的短提交只更新该 entry 内部标记，不修改异步 map/mount；保持 provider→entry mutation 的方向，与现有 Tool provider→registry 原则一致。不得有 tree/provider/work 反向回调 lifecycle gate；不得在任何同步 guard 内 await。

为保证 owner bump/revoke/dispose 与接纳/接受之间不留 check-then-act 窗口，以下是**必须单独审查的窄化 API 提案，不是现有方法**：

```rust
// PROPOSED: validate and run an internal synchronous commit while authority is locked.
OwnershipTree::with_current_binding<R>(
    &self, capability: &CapabilityId, visibility: &VisibilityScope,
    expected: OwnerGeneration, owner: &OwnerRef, lifetime: &LifetimeScope,
    commit: impl FnOnce() -> R,
) -> Result<R, OwnershipError>;

ProviderHandle::with_call_admission<R>(
    &self, expected_generation: u64, commit: impl FnOnce() -> R,
) -> Result<R, ProviderCallError>;

ProviderHandle::with_admitted_completion<R>(
    &self, expected_generation: u64, commit: impl FnOnce() -> R,
) -> Result<R, ProviderCallError>;
```

tree callback 必须同时校验 live binding 的 generation、owner、lifetime 与捕获 descriptor 一致，并检查 tombstone；不能只比 generation。当前 `OwnershipTree::register` 可覆盖 owner/lifetime 且复用当前 nonce（`src/capability/ownership.rs:334-357`），所以同代 ownership replacement 也必须因完整 binding 内容不符而拒绝；不得假设每次 register 都 bump。重注册后 generation/owner/lifetime 全部相同的 binding，在本提案中视为相同授权事实，不赋予额外撤销语义；需要强制失效时显式 bump/revoke。

admission callback 在生命周期锁内执行当前 `acquire_call` 的 Ready + generation 判据并提交 ticket；不是先调用现有 `acquire_call`、解锁后才登记工作。completion callback 允许同 generation Ready，或持有真实 pre-stop ticket 的同 generation Stopping；其他状态拒绝。所有 callback 仅更新 source 的本地 accounting/handoff。实现若不能提供该锁协议，必须保持 NO-GO，而不是靠第二次普通读取声称原子。

这是本设计所需的 admission/revocation 顺序，不是预先实现 universal transaction API；最终方法可保持 crate-private，只服务实际 source caller。tree/provider 原有独立 authority 不变。

### 6.2 Publication and activation / 发布与激活

1. 在私有准备阶段生成 channel、轻量 provider、生命周期声明 schema 和 descriptor；不生成 send operation schema，不启动资源。失败不触碰 incumbent。
2. replacement 在 source gate 下核对 incumbent，设置 retiring 并拒绝新工作；释放 gate，等 incumbent drain，调用实际 stop/cleanup。失败保留 incumbent entry/mount 和 residual，不发布 successor，也不把 provider重新标 Ready。
3. cleanup 成功后，重取 gate，预检 source revision 可 checked increment；在一个 publication cut 中替换完整 entry、更新 desired intent、提交 revision。capture 不能读到新 channel + 旧 provider，或旧 descriptor + 新 channel。
4. Bound activation 先在 source gate 下进入 Starting（因此 generation 递增），执行真实 channel.start、forwarder/mount 资源建立；最后在同 generation 上提交 Ready。可借鉴 `with_ready_commit`，但其同步 callback 只更新 entry 内标记，不能 await Tokio map/mount 锁；mount/forwarder 在 gate 外准备好，重取 source gate 验证 operation marker 后才做最终短提交。不能把 Ready 提前到实际 start 之前。
5. start 失败进入 Failed，保留真实 cleanup residual。若 mount/publication 失败，停止已建资源；不能因 channel.start 返回 Ok 就报告整个 activation 成功。

新提案要求 revision exhaustion 在不可逆 map publication **之前**返回失败；当前 `advance_source_revision` 的 panic-after-mutation 行为不是验收标准。不在本步骤修复它。

### 6.3 Capture / 同代捕获

capture 在 source gate 内验证 Bound，读取**同一个 immutable entry** 的 channel、provider、descriptor/schema/entry revision，再由实际 provider 读 generation。source token 必须属于 `self`。generation 可在 capture 后变化，capture 不是 admission；后续一律再验。

`source_revision` 是该次 registry snapshot cut；`entry_revision` 是该 entry 的 publication revision。其他 channel 发布可以推进 source revision，但不能让未变 entry 的 capture 失效。替换相同 ID 或只更新 contract 都创建新 immutable publication identity，使旧 capture stale。

### 6.4 Admission / 接纳点

真正执行前，registry 持 source gate：验证 source token、当前 entry identity/channel Arc、descriptor fingerprint/entry revision、未 retiring；进入当前 tree binding guard，验证 visibility、owner generation、descriptor owner/lifetime 与 tombstone；进入 provider admission guard，验证 `same_instance` 与 generation、Ready；最后登记唯一 ticket 与 in-flight +1。

**ticket 登记就是组合 admission 的线性化点。** 任何前置失败都不能触碰 channel.send 或创建 runtime 资源。resolve 成功、旧 `get` 返回 Arc、一次 `acquire_call` 成功，都不是这个证明。

这里接纳的是 source-local 内部工作；descriptor/schema fingerprint 只锚定该工作的生命周期 source，不验证 send payload，也不授予公开 Callable 权限。send 仍使用真实 Channel 的 typed transport 契约及既有授权，不从 lifecycle schema 推导操作许可。

执行只使用 proof 保留的 channel，不再按 ID get 一次。释放所有 gate 后才进行真实 I/O。若发送路径需 RateLimited 重试或 delivery queue，每个新的 transport attempt 都要重新准入；不得携带旧 proof 越过 stop/replacement。队列持久化/自动恢复不是本提案新授权。

### 6.5 Completion / 结果接受点

真实 work 返回后，registry 消费不可复制的 proof：先定位其原 entry，不允许以 ID 找到 successor 后写入；再次在 source gate → tree → provider → entry 边界检查同一 instance/generation、tree binding、entry 未退休。完成 verdict 和向本地 accepted-result handoff 的移动在同一短提交中完成，**该移动就是 completion acceptance 线性化点**；不采用“check 后解锁，再发布”。

- replacement/unregister 的 retiring 已先线性化：旧结果必须 `DiscardedStale`，但仍结清旧 entry 的 work accounting。
- 普通 graceful stop 允许在 Stopping、同 generation、未 retiring 的 pre-stop ticket 完成；drain 到零后才 cleanup/Stopped。没有 ticket 的调用不能借 Stopping 完成。
- owner bump/revoke/dispose 先线性化：拒绝旧结果；若 completion 先提交，则后来的 revoke 不追溯撤销已接受事实。
- source-local handoff 只定义进程内接受顺序，不是 Session durable mutation、网络推送原子提交或 exactly-once receipt。后续外部投影仍执行原有授权/可见性边界。
- 旧 completion 的拒绝、取消和 transport error 都必须结清原 entry accounting；不得 decrement successor 或改 successor 状态。

## 7. Independent validation domains / 独立代际

| 域 | 权威与比较 | 不能替代的东西 |
|---|---|---|
| Source instance / entry identity | registry token + 当前 entry/channel Arc identity | 同名 channel 或同数字 revision 不证明同 source |
| Source revision / entry revision | registry publication allocator；无关 entry mutation 不失效 | 不用它推导 provider 或 owner generation |
| Provider instance / generation | `ProviderHandle::same_instance` + live generation；每次 Starting 增长 | 相同 ProviderId、同 generation 的另一 handle 也拒绝 |
| Owner generation / binding 内容 | 同一 OwnershipTree、capability + visibility 的 generation，同时比对 live owner/lifetime | 不从 provider generation 或 producer owner 数字复制；register 覆盖不保证增代 |
| SessionEvent.seq | 既有 session event ordering | 不参与本地 channel admission，不冒充全局 cursor |

四域独立测试必须能只改变其中一项。captured generation 是 observation，不是第二份 allocator。binding 消失、revoked、disposed 必须拒绝；不能用默认 generation=0 或自动 register 来补洞。

## 8. Descriptor / schema / fingerprint derivation

**Scope decision / 已决范围：**canonical descriptor 与其 SchemaRef 只描述 source-owned channel/provider 生命周期声明。选用生命周期而非 send operation，是用户委托的 builder-check 实现决策，不是尚待二选一的审批问题；具体类型/版本与整个设计仍受用户批准门禁约束。

**Rationale / 理由与代价：**资源的发现、启停与 owner binding 不等于一次 transport operation；将两者混合会让生命周期 identity/schema 被误当成 send 的操作权限。当前 Channel/ChannelFactory 只有 Rust transport/factory 契约，没有可发布的 canonical operation JSON schema，不能从方法名、ChannelInfo、config 或 Rust 参数形状臆造一份。生命周期声明及其 publication 留在真实 source owner，准入与执行也保留 source-specific authority，不借 Tool schema/lease 补洞。这样保持 R1 有界；代价是未来 send 发布必须单独完成 operation identity、输入/输出 schema、授权和效果契约审批，不能复用生命周期声明冒充已完成。

以下为 **PROPOSED lifecycle-only canonical derivation**，不是现有 Gateway descriptor 实现：

- lifecycle endpoint identity 使用 `CapabilityId { namespace: "aleph/gateway/channels", name: channel_id }`；`name` 保留完整 ID，不按 `/` 切分后重组。source token 另外验证，不把内存地址放入公开 ID。同一个 authority 内 ID 唯一。
- lifecycle endpoint 描述候选为现有 `Resource` kind + `ProviderLifecycle` facet；**只用于此设计的声明模型，不在 R1 挂载 Resource backend，不含 Callable facet 或 send operation**。未来 send capability 必须另有 operation identity 与独立 canonical operation schema（包括真实输入/输出契约）；若投影为 Tool 也不能复用此 lifecycle ID/schema 或给此 descriptor 偷加 send 语义。
- `lifecycle_schema` 从可信 source-owned `ChannelLifecycleContract` 的版本化声明类型派生，由同一注册入口验证并固定；描述 channel/provider 生命周期声明的结构，不是 start/stop/send 调用参数或返回值 schema，也不把 live state/generation observation 变成 schema 权威。具体字段编码与首版本在实施前审查，scope 已限定；没有真实 declaration schema 就拒绝 Bound 注册，不塞 `{}`、随机 hash 或 timestamp。不得从 ChannelInfo、factory config 或 `Channel::send` 的 Rust 签名推导 operation JSON schema。
- SchemaRef.fingerprint = SHA-256(canonical JSON(lifecycle_schema))，对象键排序、数组顺序保留、primitive 编码固定；采用与现有 Tool schema canonicalization 相同的确定性原则（`src/capability/backend.rs:119-143`），但其语义是 lifecycle declaration schema hash，不是 Tool input schema hash，不复用 Tool lease。
- descriptor fingerprint 单独覆盖整个规范化 descriptor：identity/kind/schema ref/revision/owner/lifetime/visibility/services/facets/metadata，采用固定字段编码，不依赖 Debug、HashMap 迭代顺序或内存地址。它**不同于** schema fingerprint。
- `CapabilityDescriptor.revision` = 该 entry 发布时的 source revision；snapshot cursor = 捕获整个 source 的当前 source revision。descriptor/schema/publication identity 来自同一次 entry commit，不分散读取三份缓存。
- 同 channel 生命周期声明 contract 改变即新 publication/revision；即使 schema 相同而 provider 被替换，也必须改变 entry identity/revision。provider restart 可只改变 provider generation，不需要伪造 schema change。owner bump 也不自动重写 descriptor revision。send payload/transport result 不进入生命周期 schema/fingerprint；其变化不借机扩充 descriptor 的操作语义。
- owner/lifetime/visibility 的声明来自受信任 composition，并且实际 binding 必须由 OwnershipTree 验证。service/facet 只描述已批准行为；Resource/Tool/EventSource 映射的最终生产审批仍未完成。

## 9. Admitted work / drain / resource ownership

### 9.1 Proof and drain / 证明与排空

本节用真实 fixture send 证明生命周期 fence、drain 和 completion 到达实际执行边界；这些是内部执行证据，不是 send capability 发布或 operation schema 验证。

`AdmittedChannelWork` 归实际执行任务持有，覆盖从 admission 到 transport future 真正结束及 completion verdict 的整个区间。调用方超时/取消不意味着工作已结束，不能提前归零；spawned child、重试和 forwarder 也不能在未登记的后台继续持有“已释放”的资源。

entry 的 drain waiter 等待实际活动 work 清零；不是扫描 Arc strong_count，不是 sleep，也不是只读 ProviderState。work 的唯一 completion/drop cleanup 需要 exactly-once **本地记账**，这不等于外部效果 exactly-once。取消安全 guard 若无法证明 transport 已停止，保留 residual，不返回虚假的 drained。

Lifecycle owner 对 worker/forwarder/timer 的真实 cancel + join、listener/websocket/connection 的 close，以及 provider cache/database 的释放负责。`ProviderHandle` 或 OwnershipTree 的状态变化不会自动完成这些动作。当前 forwarder 持有强 Arc 的事实需要专门验证，不能用 unregister 返回值证明 cleanup。

### 9.2 Deterministic resource receipts / 确定性资源回执契约

receipts 是在**实际资源边界**记录的测试观察值，不是新 production registry，也不提供权限。每个 fixture instance 有独立 tracker，通过依赖注入观察真实 constructor/open/spawn/close/join；禁止 fixture 手工将 counter 加一来模拟资源到达。

| Receipt | 取证点与必需信息 |
|---|---|
| worker/task/thread、timer | spawn attempt、started、cancelled、joined、active；包括 inbound forwarder |
| HTTP/token client、auth/token request | client construction attempts 与请求 attempts 分开；失败尝试也计数 |
| listener/connection/websocket | bind/connect/open attempts、opened、closed、active；包括失败连接 |
| database/cache | open/init attempts、成功创建、close/drop、active；provider 专用 cache 与纯 descriptor allocation 区分 |
| admission/effect/completion | ticket minted、真实 send boundary 到达、transport result、Accepted/DiscardedStale、drain released |

使用 barrier/oneshot 确定调度，不用 sleep 碰运气。必须有正对照：显式 activation 和真实 fixture send 会改变对应 receipts，证明探针确实接在线上；缺少某类资源的 fixture 不得宣称覆盖了那类资源。不可只检查 `(state, generation)` 没变化。

### 9.3 Disabled / Discovered / Cold read matrix（提议验收）

所有格子的 zero 指 **provider-runtime attempts/active 增量为零**，不禁止 Arc、metadata、锁或内存 snapshot 的普通分配；ChannelRegistry 自身已有轻量 inbound broadcast 不等于 provider worker。

| 操作 | Discovered | Cold | Disabled |
|---|---|---|---|
| 构造声明 / 显式注册不激活 | 可见声明；zero | 可见声明；zero | 可见声明；zero，boot 不 start |
| describe / list / project | 只读同 entry 生命周期声明投影，无 Callable/send；zero | 同左；zero | 可显示 disabled，无 Callable/send；zero |
| capture_bound_channel | 只捕获 declaration proof；zero | 同左；zero | 同左；zero |
| 内部 resolve_channel_admission | NotReady，无 ticket；zero | NotReady，无 ticket；zero | NotReady，无 ticket；zero |
| 直接内部 admit / 复用旧 lease | 拒绝；zero | 拒绝；zero | 拒绝；zero |
| 将 lifecycle descriptor 用作公开 send/Callable | 不提供此操作；无 ticket、无 send；zero | 同左；zero | 同左；zero |
| 显式 activation | 才可 Starting + 资源正对照 | 同左 | 仅明确用户管理动作可激活，不由读取触发 |
| 重复/未知 ID/错误 visibility 读取 | 不注册、不认证、不连接；zero | 同左 | 同左 |

Legacy read 保留兼容信息，但 Capability resolve/admit 始终 UnboundLegacy。Ready 时内部 resolve/admit 仍须通过 §6 全部判据；Ready 不会将 lifecycle descriptor 变为 Callable，也不提供公开 send operation。Ready、Failed 的资源数可能不为零；Failed 必须报告残留，不能以状态名推导 clean。

## 10. Lifecycle / stale completion matrix（提议，不是当前实现）

| 事件 | Entry / 新 admission | 已接纳工作与 completion | 资源 / publication |
|---|---|---|---|
| replacement 同 ID | 旧 entry retiring 先提交；拒新工作 | 旧结果自 retirement 起拒收；原 accounting 排空 | drain + 实际 stop 成功才整体发布 successor；新 entry/provider identity，不凭数字相等复用 |
| unregister | 旧 entry retiring；不产生新 lease | 旧 completion stale；必须结清原 work | 成功 cleanup/unmount 后移除并增 revision；返回值不能当 receipt |
| 普通 stop | provider Stopping，拒新 admission，entry 暂不退休 | 允许真实 pre-stop ticket 的同代完成；排空后 Stopped | cleanup/join 后才报 stop 成功；entry 可保留供 restart |
| restart 同 entry/handle | 停止阶段拒新；进入 Starting 增 generation | 所有旧 generation proof 拒绝 | 新 generation start 成功再 Ready；不靠 source revision 检出 restart |
| stop/replace/unregister cleanup 失败 | 保留 incumbent、retiring/draining residual；不隐式重启 | 不把残留工作算 successor；不伪造成功 | 不发布/删除 incumbent，不提前移除其 mount；保留结构化错误及真实资源状态 |
| owner bump | source/provider 不变；旧 owner lease stale | 旧 binding-generation completion 拒绝 | 不因此停止共享 provider，不改 source revision |
| revoke capability | tree tombstone，所有 visibility binding 拒绝 | tombstone 先于接受点则 stale | 不自动 stop 整个 provider；独占 lifecycle owner 可另发显式 cleanup |
| dispose lifetime | tree 移除覆盖 binding，禁止读侧复活 | 对应 binding completion 拒绝 | shared runtime provider 不因一个 Session/Run dispose 停机；清理责任须有真实独占依据 |
| late completion 在 successor 后返回 | 旧 entry / source / provider 至少一个不匹配 | DiscardedStale；不能写 successor、降其计数或卸其 webhook | 外部 send 已发生则不可撤销，只拒接受结果，不自动补发 |
| provider Failed / generation 耗尽 | 拒绝 admission | 非有效 Ready/Stopping 同代 proof 拒收，保留 cleanup accounting | 不 wrap/saturate/reuse generation，不声称资源为零 |
| publication revision 耗尽 | 失败，不提交新 entry | 现有工作按原 proof 规则处理 | 无半发布、无 wrap；不可因失败丢 incumbent |
| caller cancel / timeout | 不另发 ticket，不自动重试 | 等真实结束/取消确认后结清；无法确认留 residual | 不能以 Future 被调用者丢弃证明网络请求没执行 |

失败保留的是 incumbent 身份和可诊断残留，**不保证仍可调用或仍为 Ready**。当前 bounded tests 对某些 fixture 的“仍 Connected”断言不是所有 provider cleanup 失败后的通用恢复承诺。

## 11. Tests / acceptance criteria（未来实施门槛）

先改写错误 provenance 的 RED 为真实 source-driven 契约测试，再观察失败并实施；不为了保留错误断言而回填 capture 字段。本步骤不编辑或运行测试，也不继承历史测试数字作为新证明。

| 类别 | 必须观察到的真实结果 |
|---|---|
| Registration provenance | 走真实 Bound 注册得到 source-minted handle；actual start/stop 使用同一 instance；独立创建的相同 ID/generation handle 不能替代它 |
| Legacy fail closed | 原 register/factory 只返回 channel 时，capture declaration 可用但 Capability resolve/admit 拒绝；无隐式 ProviderHandle::ready |
| Atomic publication | barrier 干预 replacement/contract update，capture 只见完整旧或完整新 tuple，不见混搭 |
| Source identity | 两个 registry 的同名 entry、同 revision 不可混用 |
| Unrelated mutation | 注册另一个 channel 不使未变 entry lease stale；替换本 entry 即失效 |
| Provider restart | source/owner 不变，只变 provider generation；旧 admission 与 completion 被拒 |
| Equal numbers, different instance | provider ID、数字 generation、descriptor 相同但 handle 不同，仍 stale |
| Owner independence | source/provider 不变，只 bump tree binding；拒绝旧 lease/completion；不改其他域。另测同 nonce 下 register 覆盖 owner/lifetime，完整 binding 校验仍拒绝旧 proof |
| Visibility / tombstone | 错误 visibility、missing binding、revoke、dispose 均拒绝，重复 reads 不复活 |
| Admission vs stop | barrier 决定两个顺序：stop 先则 send boundary 未到达；admit 先则 work 计入 drain |
| Completion vs revoke / replace | barrier 决定两个顺序；接受点前变更拒收，接受点后不追溯；不能只测“调用了 validate” |
| Drain | hold 真实 send，stop 不提前成功；释放后 accounting 归零，cleanup/join 发生；不死锁 |
| Stale result | replacement/unregister 后旧结果不能进入 successor handoff、不能改变 successor mount/计数 |
| Failure retention | stop 失败不发布 successor/移除 incumbent/mount；返回结构化 residual，后续不自动 Ready |
| Lifecycle declaration scope | Bound publication/describe/project 只有生命周期声明，无 Callable facet/send operation；将 operation contract 当 `ChannelLifecycleContract` 注册必须拒绝；即使 Ready 也不提供公开 send |
| Internal execution evidence | fixture send 只经 source-local work fence 到达真实 transport，结果经 completion/drain；不创建 send descriptor/operation schema，不把 lifecycle schema 当 payload validator |
| Future operation separation | R1 无 send capability；未来发布必须另审独立 operation identity/schema，不允许复用 lifecycle ID/schema 或给其 descriptor 夹带 send |
| Schema / fingerprint | 使用真实版本化 lifecycle declaration schema；缺失/占位声明拒绝；JSON object 键序变化 hash 相同，声明 schema 变化 hash 不同；descriptor-only change 改完整 hash，新 publication 必须 stale；send payload/result 不混入该 schema/hash |
| Exhaustion | provider/source checked failure 不半发布、不复用 generation/revision |
| Resource read matrix | §9.3 每格重复读并检查所有适用 receipts 零增量；activation/send 正对照真实增长；stop active=0 + joins 完成 |
| Cancel / error | transport error 不伪装成功；caller cancel 不提前清账、不暗中重试；residual 可观测 |
| All-channel contract | 至少两个不同非 Feishu 传输形态 fixture 共用注册/准入/完成路径；不写 channel-type 条件分支绕过 |
| Compatibility regression | 既有 ChannelRegistry、Tool provider/owner/lease 测试不回归；Tool proof 保持 source-specific |

实施前按项目 testing skill 和 package 指南选择精确 cargo filter；至少检查 alephcore lib 编译、上述 focused tests、既有 registry/capability 回归，按实际改动决定 fmt/clippy/workspace 验证。验证报告必须列出实际命令、RED 原因、GREEN 范围和未覆盖资源。**design 审查通过不等于 R1 通过；必须另作 gate ruling。**

## 12. Legacy compatibility / 迁移边界

1. 原 `register` / `register_for_boot` / `create_channel` 和 Channel/ChannelFactory 的 wire/pure contract 不在本步骤改动。未来先保留 Legacy entry 兼容路径，只有显式 Bound 入口启用新 proof。
2. 先盘点实际 boot、config create/update/delete、start/restart/stop、health restart、outbound send/typing/poll/retry、webhook/inbound forwarder 的调用入口。当前 boot 绕过 registry factory 的事实意味着只改 factory 无法完成迁移。
3. 在原 registry 里实现 canonical entry 与 lifecycle coupling，再用真实 source fixture 验证；不先挂 facade/MCP/Panel 来掩盖 source 缺口。
4. 逐通道审计 constructor 是否冷、start/stop 是否真实管理资源、是否有隐藏任务及 send 旁路，再明确 opt-in。不能以“implements Channel”自动判合格。
5. 兼容 get/send 等旧路径尚未统一进 admission 时，必须明确其不受新 Capability proof 保证；若它们可作用于 Bound entry，实施须路由到相同 fence 或拒绝旁路，不能留下裸 Arc 可绕 stop 的新保证。
6. 对外 read 投影由同一 canonical entry 派生；不创建第二个 channel catalog。只返回 ID/Option 的旧 API 无法表达完整 residual，新 Capability 管理入口用明确 Result，不以兼容返回值伪报成功。
7. 不增加 slot-api → Core 依赖。若未来要向纯 factory 暴露 opt-in，需单独设计依赖无关的声明契约；本提案先保留 Core/Gateway 内部可信 composition 入口。

## 13. Deferred providers / kinds / 后续范围

- Feishu 是众多通道中的后续实例；本步骤不进入 R2，不做 Feishu adapter、A/B call graph、mount 或生产消息发送。
- Telegram、Discord、Slack、Matrix、其他 Gateway channels 逐一迁移和 formal all-channel inventory 尚未完成；两个 fixture 不能代替全通道库存。
- Terminal、Memory/graph、Sandbox、Bridge 等 provider 不由此自动获得 binding 或资源保证。
- Skill、Agent、Task、Resource、EventSource、Subscription、Plugin、Hook 的可执行 backend 继续 deferred。本文 Resource endpoint 是待批准的生命周期声明模型，不是 Resource runtime mount；Tool 仍是当前唯一 callable kind。本 R1 slice 不发布 send Callable；未来 send capability 的独立 operation identity/schema 与效果契约须另审，不能塞入生命周期 descriptor。
- universal source lease、shared-provider 最后使用者策略、event replay、durable intent/result、EffectClaim producer、Safe Replay、external exactly-once、分布式 fencing 均另审。

## 14. Open questions / 审批前仍需确认

1. **独占映射粒度：**本提案的首个实现是一个 Bound channel registration 对应一个由 registry 管理的 provider lifecycle。若多个 channel 共用真正的底层 runtime，必须先给出真实 owner/共享清理契约；不得复制 handle 或加第二张 refcount owner map 草率推广。
2. **Lifecycle declaration 编码与公开投影：**“lifecycle 还是 send”已由用户委托的实现决策解决：本 slice 仅声明 source-owned channel/provider 生命周期。仍需批准 Resource / ProviderLifecycle 的具体声明映射、ID namespace、`ChannelLifecycleContract` 字段编码与 schema version 首值；不是重新开放 operation schema 选择。未来 send 的独立 operation identity/schema 在另一个 capability 设计中审批，不阻塞本 slice 的声明范围裁定；批准 source binding 不自动批准新的 backend。
3. **锁 API 最小面：**§6 的 tree/provider guarded commit 是否可保持 source-local、如何复用现有错误类型，需实现审查和死锁测试。只使用现有 generation() + acquire_call() 的解锁组合不能满足强原子语义。
4. **Cleanup residual 的恢复动作：**失败后保留并拒绝准入是本提案默认；operator retry cleanup 的入口、超时和展示格式未定，不能由 harness 自选恢复策略。没有用户批准不引入强制丢弃资源路径。
5. **真实资源仪表化：**不同 transport 的 thread/client/database/cache 分配点与 cancel/join 能力仍需 source audit；无法取得 receipt 的项必须标未证明，而不是推断为零。
6. **Admission coverage：**registry 的原 send family、retry、webhook 及 forwarder 在 Bound entry 上的统一覆盖需完整 call-site inventory；未接入部分不能带新保证发布。
7. **Owner lifetime：**现有 lifetime 轴不能区分 channel-instance scope；首 slice 只使用现有合法 binding。若确需新 lifetime 类型，应另行 architecture approval，不在本文暗中扩张。

## 15. Approval / 本步骤交付与未做项

**需要用户批准本文后，才能另行制定/执行实现步骤。方案 1 已选、builder-check 的 lifecycle-only 范围已按用户委托作出实现决策，不等于本提案所有签名、锁扩展与生命周期语义已批准。R1 在实现与验证完成、另行 gate ruling 前仍为 NO-GO / deferred；没有 R1 GREEN 结论。**

本次仅更新 `docs/superpowers/specs/2026-10-26-r1-channel-provider-binding-authority-design.md`。未修改 Rust、既有 `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md` 或任何其他文件；保留工作区原有改动。

未实现上述 types/methods、未修 RED 探针、未运行 cargo 编译/测试/QA、未改 Tool lease、未挂载 Capability backend、未注册真实 provider、未启动/停止真实通道、未发送网络请求或消息、未 commit/push。源码阅读和本文件存在/行数核对不构成实现正确性或资源隔离证明。
