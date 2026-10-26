# Capability Core Provider Contract Design

日期：2026-10-25  
状态：Phase 1 contract-first vertical slice 已实现并完成验证；Feishu production wiring 仍 deferred。  
Date: 2026-10-25  
Status: Phase 1 contract-first vertical slice is implemented and verified; Feishu production wiring remains deferred.

R0/R1 assessment (2026-10-26): R0 passed with 214 capability tests; the bounded R1 `ChannelRegistry` teardown slice is implemented and verified, but **R1 remains NO-GO / deferred overall**. The source-specific non-Tool capture/validation boundary and provider-resource receipts remain open. See [R1 Provider / Source / Lease Design Gate](2026-10-26-r1-provider-source-lease-design.md) for the current decision and evidence limits. R2 was not entered.

2026-10-26 复核：R0 通过；R1 的通用 `ChannelRegistry` replacement/unregister teardown slice 已完成 RED→GREEN，但整体 R1 仍未获 GO。下一步仍限于修订 source-owned capture contract，不进入 Feishu Rust wiring。

## 1. Intent / 意图

Aleph 的目标不是把所有现有模块搬进一个更大的注册表，而是把“外部可组合能力”的语义收敛到一个轻量 Capability Core，同时保留每个 provider 对自身运行时状态、资源和外部协议的权威。这样 Gateway channel、Terminal、Memory、Sandbox 等能力可以被发现、启动、停止、替换和订阅，而 disabled provider 不会因为 Aleph 启动而初始化线程、连接、数据库、缓存或 listener。

The design establishes a small Capability Core without turning it into a second runtime or a registry of registries. Existing subsystem authorities remain authoritative for their own instances, lifecycle, provider fencing, ownership, durable events, and external protocols.

本阶段采用 contract-first vertical slice：先定义可被多个 provider 复用的 source/provider/lifecycle 边界，再用惰性 provider fixture 验证资源隔离、generation fencing 和 late-completion 行为，最后才把 Feishu 接入为第一条真实多面 provider。

## 2. Current position / 当前状态

当前已经闭合的部分：

- 九类 `CapabilityKind` 保持稳定。
- `CapabilityFacet::{Callable, EventSource, Subscription, ProviderLifecycle, ExternalEffect}` 已加入 `CapabilityDescriptor`。
- `Scope::facet`、Tool backend 和 `ZahirFacade` 使用同一 facet predicate。
- Tool 是目前唯一具有完整 descriptor、canonical registry、provider admission、owner generation、lease validation 和 projection 的 Capability slice。
- `ProviderHandle` 已能表达 provider state、instance、generation、admission 和 live notification。
- `OwnershipTree` 已能表达 visibility-scoped owner generation、revoke、dispose 和 stale claim invalidation。

尚未闭合的部分：

- facet 目前主要是 descriptor metadata 和 Tool projection filter，不是完整的 provider runtime contract。
- `BackendLease` 仍然包含 Tool-specific canonical entry；不能直接假设它已成为所有 provider 的通用 lease。
- Gateway 的 `ChannelRegistry` 仍是 channel instance、factory、desired state、start/stop 和 webhook mount 的唯一 authority；R1 slice 已将 replacement/unregister 的 teardown fence 统一落在该 authority 内。
- Feishu 的 `ProviderHandle`、`FeishuApi`、`api_handle` 和 webhook server 仍是 Feishu 内部 authority，没有 Capability descriptor、resolve route 或 production mount。
- lazy activation 尚未形成统一生产路径；当前 Tool `ProjectionHost::mount` 是启动期 Tool projection，不是通用 provider supervisor。
- `EffectClaim` reducer 已存在，但没有生产 producer、external exactly-once、universal scheduler 或 durable Feishu recovery。
- Terminal、Memory、Sandbox 和其他七个非-Tool kind 尚未接入统一 backend。

## 3. Goals / 目标

1. 让 descriptor 能表达正交行为，而不通过伪装成 `Tool` 或新增 `CapabilityKind` 处理多面 provider。
2. 定义 descriptor source、provider instance、owner binding、lease/reference 和 projection 的关系。
3. 让 provider 的 cold/ready/replaced/stopped 状态可被 Core 观察并 fail closed，同时不复制 provider authority。
4. 保证 discovery、describe、list、project、resolve 不隐式启动 provider，不认证，不创建连接，不打开 listener，不获取 token，不启动 websocket。
5. 用一个惰性 provider fixture 验证资源隔离、replacement isolation、revoke/dispose 和 late completion。
6. 为 Feishu 后续同时表达 inbound event、outbound callable、provider lifecycle 和 external effect 建立前置契约。
7. 保持 `src/harness/**` 不变，并让每一轮新增 Core 组件都能指出被替代或收敛的旧结构。

## 4. Non-goals / 非目标

本阶段不做：

- Feishu descriptor adapter、resolve route 或 production mount。
- Feishu→Zahir production wiring。
- 新增 `CapabilityKind`。
- 第二份 callable registry、descriptor registry、handler map、enabled-state map、owner/generation/disposer map 或 durable store。
- EffectClaim producer、Safe Replay、external exactly-once、universal durable scheduler 或 durable Feishu recovery。
- Rust dylib hot unload、sidecar supervisor 或跨进程 runtime replacement。
- Terminal、Memory、Sandbox 的迁移实现。
- 让 facet metadata 自动生成 provider lifecycle 或 external effect 语义。

## 5. Anti-goals / 反目标

以下形状即使能通过测试也不接受：

- 在 Core 内复制 `ChannelRegistry` 或 `ProviderHandle` 的状态，然后用 facade glue 同步两份状态。
- 让 `describe`、`list`、`project` 或 `resolve` 通过隐式启动来“修复” cold provider。
- 把 Feishu inbound webhook 的一部分包装成 `EventSource`，再把 outbound call、lifecycle 和 external effect 拆到几个互不关联的 Capability，借此声称 Feishu 已闭合。
- 把 provider generation、owner generation、registry revision 和 `SessionEvent.seq` 合并成一个 cursor。
- 用 test-only handler map、mock registry 或旁路状态证明 production authority 已经接入。
- 在没有 effect claim、drain 和 recovery 证据时声明 `ExternalEffect` 已经安全可重放。

## 6. Selected approach / 选定路线

### Contract-first vertical slice（选定）

先定义通用 contract 和不带外部副作用的惰性 fixture，再接真实 provider。Tool 作为既有兼容 slice 保持运行；Feishu 在本阶段仍 deferred。

优点：先解决 authority、generation 和 read-side contract，能直接暴露“通用 lease 是否仍被 Tool 形状污染”的问题。代价是第一阶段不会立即提供 Feishu 用户功能。

### Feishu-first（不选）

直接为 Feishu 接 descriptor、resolve 和 mount。它会让 Feishu 的 webhook、API admission、channel replacement 和 handler ownership 反向决定 Core 形状，重现 B8.4 的 semantic masquerading 和第二 authority 风险。

### Big-bang multi-provider migration（不选）

同时迁移 Gateway、Terminal、Memory、Sandbox。它不能区分 Core contract 错误与单个 provider 迁移错误，且无法满足“新增组件同时删除旧结构”的收敛纪律。

## 7. Contract model / 契约模型

### 7.1 Descriptor and facets / Descriptor 与 facet

`CapabilityKind` 是主分类；`CapabilityFacet` 是可组合行为集合：

- `Callable`：存在明确的调用入口和输入/输出契约。
- `EventSource`：provider 能产生外部或内部事件，并有 admission/source boundary。
- `Subscription`：消费者能订阅稳定的事件或状态变化流。
- `ProviderLifecycle`：provider 自己拥有可观察、可停止、可替换的生命周期。
- `ExternalEffect`：行为会改变 capability 外部控制域，需要 effect identity、claim、drain 和 recovery 语义。

facet 只描述 capability 的行为形状。它不拥有 provider state、owner generation、descriptor revision 或 durable event。Tool 当前只声明 `Callable`。

### 7.2 Source / provider / owner / projection

| 对象 | 责任 | 唯一事实源 | Core 的使用方式 |
|---|---|---|---|
| Descriptor | identity、kind、schema、facets、reference metadata | capability source 的 canonical publication | 只读投影与过滤 |
| Source | 发布 descriptor snapshot 和 source revision | 现有 subsystem authority 或 Tool registry | Core 读取，不复制 source state |
| Provider instance | runtime 资源和当前 lifecycle | provider 自己的 `ProviderHandle` 或等价 authority | Core 读取 instance/generation 和 admission result |
| Owner binding | visibility、lifetime、owner generation、claims | `OwnershipTree` | Core 通过 binding 查验 stale/revoke |
| Projection | 对 agent/runtime/协议暴露 descriptor 或 change stream | Zahir/ProjectionHost 的派生状态 | 只能从 source snapshot 和 owner observation 构造 |
| Durable event | crash recovery、effect claim、session observation | `SessionEventStore` | 不能被 registry cursor 或 projection 取代 |

Core 不增加一个拥有所有 provider 的总表。未来的 provider source 必须能返回一个 canonical snapshot，并提供不改变 source authority 的 reference；source revision 仍归 source 所有。

Phase 1 以现有 `CapabilityBackend` 作为 read-side source contract，以现有 `ProviderHandle` 作为 provider lifecycle/admission primitive；不新增 `CapabilitySource` registry，也不在 Core 保存 provider state mirror。若实现过程中需要提取 trait，该 trait 必须是现有 authority 的无状态 facade，且必须减少重复代码，不能引入新的事实源。文档中的 `list` 指协议或消费者对 `describe` snapshot 的列表投影，不表示新增第二个列表 authority。

### 7.3 Lease/reference boundary / Lease 与 reference 边界

本阶段不把当前 Tool-specific `BackendLease { canonical_entry, descriptor, owner_generation, provider_generation }` 直接宣布为 universal lease。

本阶段先冻结以下通用不变量：

1. reference 必须包含 capability identity、descriptor revision/fingerprint、visibility/reference scope，以及 provider/owner generation observation。
2. resolve 只能在 provider 当前可用且 source snapshot 可验证时签发 reference/lease；cold、stopped、revoked、disposed 或 generation 不匹配必须 fail closed。
3. use-time validation 必须重新读取 provider generation、owner generation 和 descriptor freshness。
4. source-specific callable payload 仍由 source authority 拥有；Core 不建立第二个 handler map。
5. Tool 的 canonical entry 继续留在 Tool registry contract 中，直到第二个真实 callable provider 证明了 universal lease 的必要形状。

这个保守边界避免为了“泛化”而把 Feishu 或其他 provider 塞进 Tool 的 `CanonicalEntryLease`。

## 8. Provider lifecycle / Provider 生命周期

目标状态机：

```text
Discovered -> Cold -> Starting -> Ready -> Stopping -> Stopped
                         |          |
                         v          v
                       Failed <-----+
                         |
                         v
                      Starting  (new provider generation)
```

规则：

- `Discovered` 和 `Cold` 只声明存在或可用，不创建线程、连接、token client、listener、websocket、数据库或缓存。
- `Starting` 是唯一允许分配 provider runtime resources 的状态。
- `Ready` 才允许新的 callable admission 或 event admission。
- 每次进入 `Starting` 即分配新的 provider generation，包括随后失败的启动尝试；旧 generation 永远不能重新 admission。Every transition into `Starting` allocates a generation, including failed attempts.
- `Stopping` 拒绝新的 admission，并等待该 provider authority 声明的 in-flight work drain。
- `Stopped` 和 `Failed` 不会被 read-side 自动唤醒。
- `Disposed` 是 owner/source 的终态；旧 reference、lease 和 late completion 必须 fail closed。
- replacement 必须先使旧 instance 的 admission 无效，再发布新 instance 的 snapshot；旧 instance 的 completion 不能写入新 instance 的 authority。

Core 只规定状态和 fencing 不变量；实际资源、handler task、webhook connection 和 upstream call 的 drain 仍由 provider authority 负责。

## 9. Independent domains / 独立计数域

以下四个值永远不可互换：

| 计数域 | 语义 | 当前 authority |
|---|---|---|
| provider generation | provider instance/restart fencing | `ProviderHandle` 或未来等价 provider authority |
| owner generation | visibility/lifetime binding replacement fencing | `OwnershipTree` |
| registry/source revision | descriptor/handler publication replacement | source authority；Tool 当前为 `ToolHandlerRegistry` |
| `SessionEvent.seq` | durable session event ordering/recovery | `SessionEventStore` |

`BackendLease` 可以携带这些域的 observation，但不能成为这些域的新 owner。

## 10. Read-side and call-side effects / 读侧与调用侧副作用

| 操作 | 必须允许 | 明确禁止 |
|---|---|---|
| `describe` / `list` | 读取 canonical descriptor snapshot、source revision、owner visibility | 启动、认证、token 获取、连接、listener、websocket、runtime allocation |
| `project` | 从同代 snapshot 派生协议或 UI 投影 | 通过 projection 触发 provider lifecycle 或建立第二 authority |
| `resolve` | 校验 visibility、source freshness、provider readiness、owner/provider generation；签发 reference/lease | cold provider 自动启动、网络请求、token 获取、隐式 handler registration |
| explicit `start` | 由 lifecycle owner 进入 `Starting`，分配资源并发布 Ready | 通过 read-side 隐式触发 |
| explicit `stop` / `dispose` | 由 lifecycle owner revoke admission、drain、清理资源并发布终态 | 只撤 cache 而留下后台 work 或 accepted handler ownership |
| callable/event admission | 在 Ready/current generation 下进入 provider authority | 绕过 provider fence 或将旧 completion 写入新 instance |

当前 Tool `describe`/`resolve` 可能做 ownership reconciliation，这是既有 Tool host 的幂等 binding repair；它不能被未来 provider contract 解释成“启动 provider”。新 source 必须把 binding repair 和 provider activation 分开。

## 11. Authority non-duplication / authority 不复制表

| Authority | 继续拥有 | 本阶段不得新增的替代物 |
|---|---|---|
| `ChannelRegistry` | Gateway channel instance、factory、desired state、start/stop、webhook mount | Capability channel map、第二 enabled map、第二 disposer map |
| `ProviderHandle` | provider state、instance、generation、call/event admission | Core provider state mirror、第二 generation counter |
| `FeishuApi` | outbound/inbound call guard、token/API call admission、API drain | Core HTTP call registry、第二 in-flight call set |
| `api_handle` | derived live handle/cache | canonical provider descriptor or owner map |
| `OwnershipTree` | visibility binding、owner generation、claims、revoke/dispose tombstones | descriptor-local owner map、facade owner cache |
| `ToolHandlerRegistry` | Tool handler、Tool descriptor capture、Tool source revision | generic registry-of-registries or copied Tool map |
| `SessionEventStore` | durable session/effect event sequence and recovery evidence | projection history, registry cursor, in-memory durable store |
| Zahir/ProjectionHost | derived snapshot, filter, change stream and consumer projection | provider lifecycle authority or external effect authority |

## 12. Stale/replacement/late-completion matrix / 失败矩阵

| 情况 | 必须发生 | 不能发生 |
|---|---|---|
| provider restart | provider generation increments; old admission rejected | old reference invokes new instance |
| provider not Ready | resolve/admission fail closed | resolve auto-starts provider |
| owner bump | owner generation increments; old owner lease rejected | registry revision is used as owner generation |
| revoke/dispose | binding becomes unavailable; old lease and completion rejected | revoked capability resurrects through read-side |
| visibility mismatch | discovery/resolve/project hide or reject | caller gets a lease by guessing identity |
| descriptor replacement | source revision/fingerprint changes; old descriptor rejected | old handler is paired with new descriptor |
| channel replacement | ChannelRegistry stops/unmounts incumbent before replacement according to its policy | Core creates a second channel lifecycle authority |
| unregister | source publication disappears; existing binding semantics remain explicit and fail closed where required | unregister silently becomes a new durable recovery event |
| outbound API call | FeishuApi owns admission and drain | Core duplicates token/in-flight call state |
| webhook admission | webhook server validates/authenticates/deduplicates and hands off to its owner | read-side projection opens listener or accepts work |
| accepted handler task | owner must define join/drain responsibility before claiming complete stop | `drain` reports success while Axum-owned accepted tasks remain unjoined |
| late completion | completion carries instance/generation/effect identity and is rejected if stale | old completion mutates replacement state |

## 13. Lazy activation fixture / 惰性 provider fixture

The fixture is a contract test source using the same descriptor/source/provider boundaries intended for production. It is not a parallel production authority and does not stand in for Feishu.

It must expose observable counters or receipts for:

- runtime allocation;
- listener/connection creation;
- authentication/token acquisition;
- start/stop transitions;
- provider generation;
- admitted and rejected calls/events;
- drain completion;
- stale completion rejection.

Required scenarios:

1. construct/discover/describe/list/project without runtime allocation;
2. resolve while Cold without runtime allocation and with fail-closed result;
3. explicit start creates resources exactly once and publishes Ready;
4. callable/event admission works only for the current generation;
5. stop rejects new work and drains admitted work;
6. restart creates a new generation and rejects old references;
7. owner bump/revoke/dispose invalidates old references independently of provider generation;
8. replacement rejects late completion from the old instance;
9. disabled source has no worker, listener, connection, token or timer allocation;
10. projection and read-side operations remain side-effect free after replacement and after dispose.

## 14. Feishu entry criteria / Feishu 后续接入前置条件

Feishu cannot enter production Capability routing until all of the following are source-backed:

- one descriptor source is derived from the existing Gateway authority without a second descriptor registry;
- the descriptor can express inbound event source, outbound callable behavior and provider lifecycle without Tool masquerading;
- provider instance/generation comes from the existing `ProviderHandle`;
- channel identity, desired state, start/stop and webhook mount remain owned by `ChannelRegistry`;
- owner generation and visibility come from `OwnershipTree`, not Feishu-local state;
- resolve/use-time validation has a defined non-Tool lease/reference shape;
- webhook accepted task ownership and drain are explicit;
- outbound API admission and provider restart isolation are observable;
- `ExternalEffect` is withheld until EffectClaim producer, drain, recovery and late completion are closed;
- describe/list/project/resolve have no start/auth/network/listener/token/websocket side effect;
- a real production mount can be added without adding a Feishu adapter or second registry.

If any item remains open, Feishu remains deferred and the implementation stops at the first unclosed contract.

## 15. Implementation phases / 实施阶段

### Phase 0 — design approval

- Review this document with owner.
- Resolve any builder-check questions that affect architecture.
- Invoke the writing-plans workflow only after this document is approved.

### Phase 1 — generic contract extraction

- Treat the existing `CapabilityDescriptor` + `Scope` + `CapabilityBackend` read surface, `ProviderHandle` lifecycle/admission primitive, and `OwnershipTree` binding as the first generic contract.
- Keep Tool behavior compatible and preserve its canonical registry/lease path.
- Do not change `BackendLease` into a universal lease in this phase; keep Tool `CanonicalEntryLease` source-specific until a second real callable provider proves the required shape.
- Do not add a Core-wide `start`/`stop` authority in this phase. Explicit lifecycle remains owned by the provider authority; the fixture exercises that contract through the same provider primitive.
- Add explicit read-side no-start and generation-domain tests in the existing capability test modules.

本 slice 已实现：`CapabilityBackend::snapshot(&Scope) -> CapabilitySnapshot` 现在是 source snapshot contract；`ZahirFacade::describe` 使用该 contract；cold provider 的 `describe`、`project`、`resolve` 不会启动 provider；provider restart、owner bump 与旧 generation late completion 已有独立测试。该 slice 没有新增 provider registry、Core lifecycle authority 或 universal lease。

Implemented in this slice: `CapabilityBackend::snapshot(&Scope) -> CapabilitySnapshot` is now the source snapshot contract; `ZahirFacade::describe` consumes it; cold-provider `describe`, `project`, and `resolve` do not start the provider; provider restart, owner bump, and stale-generation late completion have independent tests. This slice adds no provider registry, Core lifecycle authority, or universal lease.
- Candidate implementation surfaces are `src/capability/descriptor.rs`, `src/capability/facade.rs`, `src/capability/backend.rs`, `src/capability/provider.rs`, `src/capability/ownership.rs`, and `src/capability/zahir_facade.rs`; any new file must be justified as a zero-authority contract or test surface.

### Phase 2 — lazy provider vertical slice

- Mount the fixture through the same contract used by production sources where possible.
- Prove Cold/Ready/Stopped/Failed/restart semantics.
- Prove replacement, revoke/dispose and late completion.
- Prove disabled resource isolation with observable receipts.

### Phase 3 — Feishu contract re-gate

- Re-run B8.4 A/B using the Phase 1 contract.
- Produce source-backed descriptor, identity, owner, provider, lease, effect and drain mapping.
- Stop immediately if any authority or effect boundary remains unclosed.
- Only after A and B independently pass may production Feishu wiring be proposed.

### Phase 4 — remaining providers

- Migrate one subsystem at a time: Terminal, Memory, Sandbox, then other Gateway channels.
- For each migration, delete or retire the replaced structure in the same change set.
- Do not claim the whole Core is converged until every external surface has a source-backed authority mapping.

## 16. Acceptance criteria / 验收标准

This design phase is complete only when:

- the design is approved;
- builder check has no unresolved architecture question, or every question has an owner answer;
- no Rust or production configuration was changed during design;
- the implementation plan names exact files, symbols, tests and authority boundaries;
- the first implementation slice can be tested without Feishu production wiring;
- the plan can prove that read-side operations never start disabled providers;
- the plan keeps `src/harness/**` untouched;
- the plan explicitly identifies what remains deferred rather than implying universal Capability closure.

本设计稿完成后，下一步不是直接写 Rust，而是先根据 builder check 修订文档，等待 owner approval，再进入 writing-plans 和实现阶段。
