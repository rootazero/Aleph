# SessionCapability 设计 / SessionCapability Design

**状态 / Status:** 供评审的设计提案；不代表 Phase 4 已实现这些接缝。  
**范围 / Scope:** 说明 Core 中的会话能力模型，以及桌面端 `SessionManager` 如何成为其客户端；不实现代码、不改变 RPC 契约。  
**结论 / Decision:** 不把 `desktop/nsh-shell` 的 UI `SessionManager` 本身注册进 Core。将 Core 所有的单会话能力建模为 `CapabilityKind::Resource`，NSH 的 `SessionManager` 改为该能力的桌面客户端与 UI 投影。每个会话能力是 `LifetimeScope::Session`，由该会话拥有；进程级的连接、Capability facade 和 UI manager 仍是 Runtime/进程生命周期。

**The desktop UI `SessionManager` must not itself be registered in Core.** Model the Core-owned, per-session domain object as a `CapabilityKind::Resource`; make NSH's `SessionManager` its client and UI projection. Each session resource is session-scoped and owned by that session. The desktop connection, capability facade, and UI manager remain process/runtime-lived.

## 1. 事实与边界 / Facts and boundaries

Phase 3 的 `SessionManager` 位于 `desktop/nsh-shell/src/session.rs`。它目前持有可变的单一 `session_id`、消息列表、生成标志，以及一个 `mpsc` 事件发送端；`create_session` 生成本地 UUID，`send_message` 启动模拟回复，`load_messages` 是空实现。它是 UI 开发用的 mock 状态容器，而非持久化会话的权威实现（`desktop/nsh-shell/src/session.rs:39-48,70-119,183-188`）。若直接把该对象挂进 Core Capability 注册表，就会跨越 R1/R6：桌面侧同时变成第二个会话大脑，服务器状态与窗口缓存可能分叉。

Phase 3's `SessionManager` is a UI-development mock, not the authority for persisted conversations: it keeps one mutable local ID, message list, generation flag, and an `mpsc` event sender; it creates a local UUID, simulates replies, and has a no-op history load (`desktop/nsh-shell/src/session.rs:39-48,70-119,183-188`). Registering this desktop object directly in Core would violate R1/R6 by creating a second session authority that can diverge from server state.

Core 已有两块相关契约但职责不同：`src/session/service.rs` 的 `SessionService` 操作 append-only session event log（`attach/get_events/emit_event/subscribe/wake/detach`）；Gateway 暴露面另有 `chat.send`, `chat.history`, `sessions.new`, `sessions.list` 与 `events.subscribe`。`SessionService` 不是 Gateway 的聊天发送接口；设计应复用这两端，而不是另建消息存储。`CapabilityKind` 已含 `Resource`，而 `LifetimeScope` 实际枚举是 `Runtime/Session/Run/Task/External`，没有名为 `Process` 的变体（`src/capability/descriptor.rs:25-35`; `src/capability/ownership.rs:29-40`）。

Core already has two distinct relevant contracts: `SessionService` in `src/session/service.rs` operates on the append-only event log (`attach/get_events/emit_event/subscribe/wake/detach`), while the Gateway separately exposes `chat.send`, `chat.history`, `sessions.new`, `sessions.list`, and `events.subscribe`. `SessionService` is not the Gateway's chat-send API. Reuse these boundaries instead of creating another message store. `Resource` already exists as a kind, and the actual lifetime enum is `Runtime/Session/Run/Task/External`—there is no `Process` variant (`src/capability/descriptor.rs:25-35`; `src/capability/ownership.rs:29-40`).

还有一个必须先解决的类型接缝：`SessionService::SessionId` 是 `routing::session_key::SessionKey`；`OwnerRef::Session` 和 `VisibilityScope.session` 使用的则是 `acp::manager::SessionKey`。两者结构和语义不同，仓库内尚未找到映射。因此下文的 Session owner/visibility 是目标语义，不是可直接编译的现有赋值；不得把 UUID 或未经验证的字符串塞进其中冒充共享身份。

One identity seam must be resolved first: `SessionService::SessionId` aliases `routing::session_key::SessionKey`, but `OwnerRef::Session` and `VisibilityScope.session` use `acp::manager::SessionKey`. They have different structures and semantics, and no conversion was found. The owner/visibility model below is the target semantics, not a directly assignable current type. Do not pass a UUID or unverified string as if it were a shared canonical identity.

## 2. Kind 决策 / Capability kind

**选 `CapabilityKind::Resource`，不新增 kind，也不选 `Agent` / `Task`。** 单个 Session 是有身份、可读写、可订阅的对话状态及其事件历史；它本身不负责自主推理（所以不是 Agent），也不是被调度执行的一次工作项（所以不是 Task）。`Resource` 能表达“这是被操作/观察的会话对象”，具体操作行为交给独立 trait 和 facets。新 kind 只会重复表达现有 kind + facet 的组合，并扩张当前封闭的 kind 枚举与所有匹配点。

**Choose `CapabilityKind::Resource`; add no kind and do not use `Agent` or `Task`.** A session is identified, readable/writable conversational state with an event history. It does not itself reason autonomously (not an Agent), nor is it one scheduled unit of work (not a Task). `Resource` describes the object being operated on or observed; operations belong to the trait and facets. A new kind would duplicate the existing kind-plus-facet model and expand a closed enum and its match sites.

该选择把 Capability 身份与行为分开：kind 表示会话是资源；`Callable` / `EventSource` 描述接口行为。`CapabilityDescriptor` 当前已有这两个正交轴（`src/capability/descriptor.rs:38-52,119-145`）。Capability ID 建议使用 `aleph/sessions/<opaque-stable-session-id>` 命名空间；名称不得泄漏工作目录或消息内容，实际访问仍须由 ownership/visibility 与 Gateway 授权控制。

This keeps identity separate from behavior: the kind says the session is a resource; facets describe callable/event behavior. `CapabilityDescriptor` already models these as orthogonal axes (`src/capability/descriptor.rs:38-52,119-145`). Use a namespaced ID such as `aleph/sessions/<opaque-stable-session-id>`; do not expose filesystem paths or message content in the name, and continue enforcing access through ownership/visibility and Gateway authorization.

## 3. Facets 决策 / Capability facets

| Facet | 决定 | 理由 / Rationale |
|---|---|---|
| `Callable` | 是，目标接口 | `attach/history/send/detach` 是可调用操作；`send` 会触发生成。**但当前 Zahir 没有 invoke 方法**，facet 元数据本身不会分发调用。 |
| `EventSource` | 是 | Session 可产生有序事件并支持读取/订阅；Core `SessionService::subscribe` 已提供事件日志广播接缝。 |
| `Subscription` | 否 | 这是事件源，不是代表一个订阅实例/订阅管理句柄的 capability。仅“允许订阅”不足以声明此 facet。未来若独立注册可暂停/续订的 subscription handle，再为该对象声明。 |
| `ProviderLifecycle` | 默认否 | capability 的 `attach/detach` 或新建/关闭聊天语义，不等于启动/停止 provider。只有 capability 真的由 `Zahir::activate` 管理 provider 实例生命周期时才标记；Memory Resource 的生命周期 facet 是具体范例，不是所有 Resource 的默认值。 |
| `ExternalEffect` | 是（含 send 时） | `send` 会发起真实模型运行并写入对话状态。该 facet 应使消费端意识到副作用；它不提供 exactly-once 保证。 |

| Facet | Decision | Rationale |
|---|---|---|
| `Callable` | Yes, intended | `attach/history/send/detach` are operations and `send` triggers generation. **Zahir currently has no invoke method**; a facet alone does not dispatch calls. |
| `EventSource` | Yes | A session emits ordered events and supports reads/subscription; Core `SessionService::subscribe` is the existing event-log seam. |
| `Subscription` | No | This capability is the source, not a subscription instance/management handle. “Can be subscribed to” alone is insufficient. Add it only to a separately registered resumable/pauseable subscription object if such a contract is designed. |
| `ProviderLifecycle` | No by default | Session `attach/detach` or chat create/close semantics are not provider start/stop. Mark it only when `Zahir::activate` actually owns provider-instance lifecycle. Memory Resource's lifecycle facet is an example, not the default for every Resource. |
| `ExternalEffect` | Yes when `send` is included | `send` starts a real model run and mutates conversation state. This facet signals side effects; it does not guarantee exactly-once execution. |

现有 `Zahir` 只提供 `describe/resolve/validate_lease/activate/subscribe/project`；`BackendLease` 是验证凭据，不是 handler 或调用器（`src/capability/facade.rs:330-336`）。所以 Phase 4 若只实现描述/解析，暂时应仅发布已真实实现的 facets；在 invoke/执行接缝建立前，不得宣传 `Callable` 已可通过 Capability 执行。

`Zahir` currently exposes only `describe/resolve/validate_lease/activate/subscribe/project`; a `BackendLease` is validation evidence, not a handler or invoker (`src/capability/facade.rs:330-336`). If Phase 4 implements discovery/resolution only, publish only facets that are actually implemented. Do not claim Capability invocation for `Callable` until an execution seam exists.

## 4. 生命周期、所有者与共享 / Lifetime, owner, and sharing

**单个会话 Resource：`LifetimeScope::Session`；所有者目标为 `OwnerRef::Session(canonical_session_key)`。** 其有效期应覆盖此会话的 event log/epoch，而不是 NSH 窗口生命周期。桌面 `SessionManager`、Gateway 连接和 provider host 是进程级对象，对应 `LifetimeScope::Runtime`（“Process-scoped”的现有名称）。不要为了表达进程而添加新 `Process` 变体。

**Per-session Resource: `LifetimeScope::Session`; target owner: `OwnerRef::Session(canonical_session_key)`.** Its validity should follow the session event log/epoch, not the NSH window. The desktop manager, Gateway connection, and provider host are process-level objects, represented by `LifetimeScope::Runtime` (“process-scoped” in product language). Do not add a `Process` variant for this distinction.

关闭一个 session 时不可调用 `OwnershipTree::dispose(LifetimeScope::Session)`：它按 lifetime 层级作用于所有 Session 以及更短 scope 的绑定，不是针对一个 `SessionKey` 的 dispose。单会话撤销必须基于唯一 capability ID 做精确 revoke；重新打开/epoch 变化应有新的稳定 capability ID。还需验证 provider entry 的清理顺序与 ownership revoke 一致，避免残留 descriptor 或 tombstone 后复用旧 ID（`src/capability/ownership.rs:381-431,754-835`）。

Do not call `OwnershipTree::dispose(LifetimeScope::Session)` to close one session: it acts on every binding at Session and shorter lifetimes, not one `SessionKey`. Revoke one session by its unique capability ID; a new session/epoch needs a new stable capability ID. Provider-entry cleanup must also be ordered with ownership revocation so descriptors do not linger or reuse a tombstoned ID (`src/capability/ownership.rs:381-431,754-835`).

`OwnerRef` 描述一个 capability binding 的权威所有者，不是允许访问的 agent 列表。一个 Core session/event log 可以作为该绑定的单一 owner；多个 agent/客户端是否能共享同一个 session，应由 Gateway 的调用者身份、session-key 路由和授权策略决定，而不是把多个 agent 塞进 `OwnerRef`。故可共享，但不得默认开放；同一 session 的并发写入/顺序应遵守现有 actor/event-store 语义。身份模型对齐前不要注册 per-session binding：当前 `OwnerRef::Session`/`VisibilityScope.session` 的 ACP key 与服务的 routing key 不同。

`OwnerRef` names the authority that owns one binding; it is not a list of agents allowed to access it. One Core session/event log can own the binding. Whether multiple agents/clients may share it is decided by Gateway caller identity, session-key routing, and authorization—not by putting multiple agents into `OwnerRef`. Sharing can be supported, but must not be open by default; concurrent writes/order follow the existing actor/event-store semantics. Do not register per-session bindings until the ACP-vs-routing key mismatch is resolved.

## 5. Services 关系 / Service relations

初始 descriptor 的 `services` 建议为空。Gateway transport、RPC client 和 `SessionService` 是实现依赖，不是现有 capability graph 中已注册的服务 ID；虚构 `Requires` 边不会让它们自动连接。若以后 event store 被暴露为独立 Resource capability，可在确认其稳定 ID、同 visibility 下的 resolve 语义和实际依赖消费路径后添加 `Requires`。除非已有另一项能力真实声明它提供的服务，不添加 `Provides`；不同 session 可以并存，也不因共享而添加 `Conflicts`。关系只是纯数据边，不包含服务数据且不可调用（`crates/aleph-slot-api/src/capability.rs:16-31`）。

Start with an empty `services` list. Gateway transport, RPC client, and `SessionService` are implementation dependencies, not registered service IDs in the current capability graph; inventing a `Requires` edge will not wire them. If the event store later becomes a separately registered Resource, add `Requires` only after its stable ID, same-visibility resolution, and actual consumption path are established. Add `Provides` only when another concrete capability declares that service; sessions may coexist, so sharing does not imply `Conflicts`. These relations are pure data edges, carry no service payload, and are not invocable (`crates/aleph-slot-api/src/capability.rs:16-31`).

## 6. 契约草案 / Contract sketch

建议在 Core 增加 per-session facade trait；它复用现有 `SessionService` 与 Gateway chat API，不拥有第二份 event store。类型名以下表示设计边界：实现时应直接复用 Gateway 的 request/response 类型，或在公共 session API 中定义一次，不能在桌面与 Core 各复制一份。

Add a per-session Core facade trait that reuses `SessionService` and the Gateway chat API without owning a second event store. The names below show the design boundary: implementation must reuse Gateway request/response types or define them once in the public session API, not duplicate schemas in desktop and Core.

```rust
#[async_trait]
pub trait SessionCapability: Send + Sync {
    fn session_id(&self) -> &SessionId;

    async fn attach(&self) -> Result<SessionHandle, SessionCapabilityError>;
    async fn history(
        &self,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionCapabilityError>;
    async fn send_message(
        &self,
        request: ChatSendRequest,
    ) -> Result<ChatSendReceipt, SessionCapabilityError>;
    async fn subscribe(
        &self,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionCapabilityError>;
    async fn detach(&self) -> Result<(), SessionCapabilityError>;
}
```

`SessionId` 与历史/订阅/attach 应对齐现有 `src/session/service.rs`；`send_message` 走实际 Gateway `chat.send`，不要伪造一个 `SessionService::send_message`。`detach` 表示分离当前 consumer/actor handle，不代表删除 durable history。统一错误类型应区分 session store 错误、RPC/transport 错误、取消与过期 lease；不得把 channel close 当成成功完成。`SessionEventRecord` 的序号是 UI 重连与补历史的游标；旧 mock 的 `MessageDelta` 可以是 UI projection，但不能成为第二个可恢复权威。

`SessionId` and history/subscription/attach align with `src/session/service.rs`; `send_message` uses the real Gateway `chat.send`, not a fabricated `SessionService::send_message`. `detach` detaches the consumer/actor handle; it does not delete durable history. A unified error type distinguishes store, RPC/transport, cancellation, and stale-lease failures; a closed channel must not be reported as successful completion. `SessionEventRecord` sequence numbers are the cursor for UI reconnect/history recovery. The old `MessageDelta` may remain a UI projection but cannot become a second recoverable authority.

该 trait 不是 `CapabilityBackend`：backend 负责 `lookup/enumerate/snapshot`；provider implements behavior/lifecycle；`Zahir` 负责发现和 lease 验证。若 Capability 要直接 invoke，必须先设计一个由 owner 决定执行且在调用时校验 lease 的 dispatch 接缝；否则 UI 的 typed RPC client 是唯一执行路径，Capability 只做可发现性/授权锚点。

This trait is not a `CapabilityBackend`: a backend provides `lookup/enumerate/snapshot`; a provider implements behavior/lifecycle; `Zahir` handles discovery and lease validation. Direct Capability invocation requires a separate owner-controlled dispatch seam that validates the lease at call time. Otherwise the typed RPC client remains the only execution path, while the capability is a discovery/authorization anchor.

## 7. 注册与发现 / Registration and discovery

复用现有 `ProviderRegistry` + `ProviderBackendAdapter`，不要新建并行 `CapabilityRegistry` 或重复的 owner/handler map。Session provider 以 `ProviderRegistration` 注册：descriptor kind `Resource`、namespace `aleph/sessions`、session lifetime、目标 Session owner、严格受限 visibility；`ProviderHandle` 和 activator 只在明确需要生命周期管理时加入。生产 host 必须将注册与 Memory 共用的 provider registry 组合进同一个 `ZahirFacade`，而不是单独在 Tauri 壳中维护另一个注册表（`src/capability/provider_registry.rs:28-54,84-141`; `src/capability/zahir_facade.rs:122-150`）。

Reuse `ProviderRegistry` and `ProviderBackendAdapter`; do not add a parallel `CapabilityRegistry` or duplicate owner/handler map. Register a session provider through `ProviderRegistration`: `Resource` kind, `aleph/sessions` namespace, session lifetime, canonical session owner, and restricted visibility. Add a `ProviderHandle`/activator only when lifecycle management is actually required. Production startup must compose the registration into the same provider registry/Zahir facade as Memory, not maintain another registry in the Tauri shell (`src/capability/provider_registry.rs:28-54,84-141`; `src/capability/zahir_facade.rs:122-150`).

实现前置条件：

1. 对齐 session identity：统一 `OwnerRef::Session` 与 `VisibilityScope.session` 到 canonical routing SessionKey，或提供由 Core 所有、可测试、无歧义的转换。不要以 caller-provided name 建 owner。
2. 明确动态生命周期：当前 `ProviderRegistry` 有注册与 revision，但无 unregister/change stream；`Zahir::subscribe` 目前订阅 Tool registry 广播。若要求每个会话动态出现/消失并让 subscriber 收到变更，先补齐 provider publication/unregister 与 snapshot-anchored subscription，再挂 session provider。不能把一次 `describe` 命中写成支持动态订阅。
3. 接入 Core 启动并复用同一 `OwnershipTree`、provider registry 与 Zahir facade。`SessionManager` 通过 Gateway `AlephClient` 调用业务 API并订阅事件，将结果投影到 UI；不持有服务器侧 capability authority。
4. capability 注册 ID 必须反映 session epoch；关闭时精准 revoke、取消该 session 的 stream/task，并保留 durable history 的政策另行遵循现有 Gateway 行为。

Implementation prerequisites:

1. Align session identity: use the canonical routing SessionKey in `OwnerRef::Session` and `VisibilityScope.session`, or provide an unambiguous, Core-owned tested conversion. Do not derive ownership from a caller-supplied display name.
2. Define dynamic lifecycle: `ProviderRegistry` currently supports registration/revision but no unregister/change stream; `Zahir::subscribe` subscribes to Tool registry broadcasts. If sessions must appear/disappear dynamically with subscriber notifications, first add provider publication/unregister and snapshot-anchored subscription. A successful `describe` is not proof of dynamic subscription support.
3. Integrate at Core startup with the same `OwnershipTree`, provider registry, and Zahir facade. NSH `SessionManager` calls business APIs through the Gateway `AlephClient`, subscribes to events, and projects results into UI; it does not own server capability authority.
4. Include session epoch in capability identity. Closing precisely revokes the session binding and cancels its stream/task; durable-history retention remains governed by existing Gateway behavior.

**发现/调用示例（目标形状） / Discovery and call example (target shape):**

```rust
let snapshot = zahir.describe(Scope {
    kind: Some(CapabilityKind::Resource),
    namespace: Some("aleph/sessions".into()),
    facet: Some(CapabilityFacet::EventSource),
    visibility: caller_scope,
});
let descriptor = snapshot.capabilities.iter()
    .find(|d| d.id == requested_session_capability_id)
    .ok_or(SessionUiError::NotVisible)?;
let lease = zahir.resolve(Reference {
    id: descriptor.id.clone(),
    visibility: caller_scope.clone(),
})?;
zahir.validate_lease(&lease)?;
// Execution remains a typed SessionCapability / Gateway client call.
let mut events = session_client.subscribe(session_id).await?;
```

此例中的 `describe/resolve/validate_lease` 是现有能力面；最后一行是独立 typed client，不能替换成不存在的 `zahir.invoke`。实现时要检查 descriptor ID 与传入 `session_id` 一致，并在每次副作用前按需要重验 lease。Capability descriptor 可能对不同 visibility scope 投影；caller scope 不能由 UI 输入自行扩大。

The first three operations in this example are existing capability APIs; the final line is a separate typed client, not a nonexistent `zahir.invoke`. Implementation must ensure descriptor ID matches the supplied `session_id` and revalidate the lease before side effects as required. Descriptors may be projected under different visibility scopes; the UI must not widen caller scope.

## 8. 风险、取舍与验证 / Risks, trade-offs, and validation

**主要风险 / Main risks**

- **双权威 / Split authority:** 若 UI message vector 被继续当作历史真值，会与 SessionService/event store 分叉。UI 消息只能由真实事件投影，可丢弃并从游标恢复。 / If the UI vector remains history authority, it can diverge from the event store. Treat it as a disposable projection reconstructed from events/cursors.
- **伪安全 / False security:** `CapabilityFacet` 是描述数据，不等价于 invocation gate；`OwnerRef` 也不是 caller ACL。实际调用必须经过 Gateway 授权并绑定 session identity。 / Facets are metadata, not invocation gates; `OwnerRef` is not a caller ACL. Calls must still pass Gateway authorization bound to canonical session identity.
- **动态注册缺口 / Dynamic-registration gap:** Provider changes are not currently streamed by Zahir, and provider entries cannot currently be unregistered. Dynamic per-session registration requires this source/wire lifecycle work first. / Zahir does not currently stream provider changes, and provider entries cannot currently be unregistered. Dynamic per-session registration requires fixing this lifecycle seam first.
- **副作用重复 / Duplicate side effects:** `chat.send` may start model work; retry/idempotency/exactly-once are not solved by this design. Preserve Gateway request IDs and existing semantics; do not claim durable exactly-once. / `chat.send` can start model work; this design does not solve retries/idempotency/exactly-once. Preserve Gateway request IDs and do not claim durable exactly-once.
- **身份类型冲突 / Identity type mismatch:** ACP session key versus routing session key blocks a sound owner/visibility binding until unified. / ACP and routing session keys block a sound owner/visibility binding until unified.

**验证门槛 / Acceptance gates**: (1) capability discovery returns only the intended session under the caller scope; (2) stale/revoked lease cannot reach `send`/`subscribe`; (3) event sequence resumes history without gaps/duplicates under the declared Gateway contract; (4) closing one epoch leaves other sessions intact and does not call scope-wide `dispose(Session)`; (5) a second authorized client sees shared session state only when policy allows it; (6) provider register/unregister subscription updates are anchored to a consistent snapshot; (7) real RPC integration uses current `chat.send/history`, `sessions.new/list`, and `events.subscribe`, not Phase 3 mock method names.

**Acceptance gates:** (1) discovery returns only sessions visible to the caller; (2) stale/revoked leases cannot reach `send`/`subscribe`; (3) event cursors resume history without gaps/duplicates under the Gateway contract; (4) closing one epoch leaves other sessions intact and never invokes scope-wide `dispose(Session)`; (5) a second authorized client shares state only when policy permits; (6) provider registration changes are emitted against a consistent snapshot; and (7) integration uses current RPC names, not Phase 3 mock names.

**未做 / Not done:** 本文未实现 trait、RPC client、provider registry 事件/删除、session identity 统一或真机验证；未定义多 agent 并发写入/冲突解决策略，也未承诺 send 的 exactly-once。上述均是实现/评审阶段的明确待办，不应在 Phase 4 验收中假定已完成。

**Not done:** This document does not implement the trait, RPC client, provider-registry change/removal support, session-identity alignment, or real-device validation. It does not define multi-agent write-conflict resolution or promise exactly-once sends. These remain explicit implementation/review tasks and must not be assumed complete in Phase 4.
