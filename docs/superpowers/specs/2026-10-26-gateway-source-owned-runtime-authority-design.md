# Gateway Source-owned Runtime Authority — 架构增补 / Architecture addendum

日期 / Date: 2026-10-26

**状态 / Status:** 架构扩展方向已获用户授权；本文规定待实现的边界，不是实现完成或 R1 GO 的证据。The user authorized expanding Gateway's source-owned runtime authority. Everything marked proposed below remains unimplemented; R1 stays **NO-GO / deferred** until implementation, independent verification and a separate gate ruling.

## 1. Decision and precedence / 决策与优先级

**Everything externally composable is a Capability.** 扩展现有 `ChannelRegistry` 及其 canonical entry，使其成为唯一 Gateway source-owned runtime authority；不在它旁边建立第二个 runtime/registry。Extend the existing source, not a parallel supervisor. It owns the complete local chain: source/entry identity → explicit activation → admission → actual transport → completion acceptance → drain → cleanup/publication.

本增补替代旧设计中“不能新增 admitted-work authority、因此遇到缺口就停止”的限制；现在允许**在原 registry 的原 entry 内新增这些职责**。它不推翻 ProviderHandle、OwnershipTree、Tool 或 durable store 的既有权威。This is a narrowly authorized expansion, not permission to add a generic runtime framework.

Read with:
- [R1 provider/source/lease gate](2026-10-26-r1-provider-source-lease-design.md): historical evidence and outstanding gate.
- [Earlier channel binding design](2026-10-26-r1-channel-provider-binding-authority-design.md): provenance, lifecycle-only declaration and independent validation domains remain valid.
- [Earlier implementation plan](../plans/2026-10-26-r1-channel-provider-binding-authority-implementation-plan.md): Tasks 1–3 have current source implementations; its unexecuted Tasks 4–9 are replaced by the [new staged plan](../plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md). Do not repeat completed primitives or execute both plans.

**Assumptions / 明示假设：**首阶段一个 Bound channel 对应一个独占 provider runtime；只通过测试组合启用 Bound，不修改生产 boot/config opt-in。共享底层 runtime 必须先命名真实 owner 与 cleanup 契约，不能用多份 provider handle/新 refcount map 猜测。The first runtime is in-process; terminal, memory graph, sandbox and other sources are future consumers of the principle, not automatically governed by Gateway.

**Trade-offs / 代价：**registry 内部同步、任务归属与兼容审计更复杂；强 drain 可能无限等待不可取消的真实 I/O，不能用 timeout 伪造资源已释放。未来公开操作、进程隔离和动态库卸载需要独立设计。Lazy runtime allocation does not remove compiled adapter code from the executable or guarantee RSS return to the OS. Error containment is not isolation from process abort, native crashes or OOM.

## 2. Current evidence / 当前源码事实

以下锚点来自本增补编写时的工作树；未来会移动。These are inspection results, not new test results.

| Fact / 事实 | Source / 源码 |
| --- | --- |
| One map already holds Legacy/Bound canonical entries; Bound stores declaration, provider and tree reference | `src/gateway/channel_registry.rs:61-81`, `src/gateway/channel_registry.rs:254-285` |
| Cold Bound registration source-mints the provider and commits entry/intent/revision without starting it | `src/gateway/channel_registry.rs:657-724` |
| Capture and exact-scope declaration snapshot exist, but no runtime/work owner is integrated | `src/gateway/channel_registry.rs:728-779` |
| Bound legacy execution currently rejects; list/status paths still inspect channel objects | `src/gateway/channel_registry.rs:781-878`, `src/gateway/channel_registry.rs:1671-1739` |
| Legacy lifecycle holds source gate over real start/stop; forwarder is detached | `src/gateway/channel_registry.rs:898-963`, `src/gateway/channel_registry.rs:1561-1626` |
| Legacy publication increments after mutation and panics on exhaustion | `src/gateway/channel_registry.rs:642-652` |
| Provider admission/completion mutex-held guards already exist; they own no work or resources | `src/capability/provider.rs:336-398` |
| Tree full-binding guard already checks tombstones, generation, owner and lifetime under one mutex | `src/capability/ownership.rs:418-460` |
| Tree register can replace owner/lifetime without advancing nonce | `src/capability/ownership.rs:347-372` |
| Actual Webhook constructor eagerly constructs reqwest client; start builds handler; stop only drops handler | `src/gateway/interfaces/webhook/mod.rs:92-118`, `src/gateway/interfaces/webhook/mod.rs:160-208` |
| Configuration and boot do not use only registry factories | `src/gateway/handlers/channel.rs:469`, `src/bin/aleph-server/commands/start/builder/subsystems.rs:457-567` |

结论 / Finding: 已有冷态 association 和独立 guards 不是 execution proof。The missing runtime responsibilities now have an explicit authorized home: the existing entry and its owning registry.

## 3. Authority partition / 唯一权威分工

中文摘要：原 registry 独占 source/entry、操作标记、真实任务与资源、接纳票据、结果接受和排空；ProviderHandle 只管生命周期与代际，OwnershipTree 只管 owner binding 与 tombstone。资源回执只做测试观察，不能拿来决定生产权限。下表明确每种事实的唯一归属。

| Authority / 权威 | Sole facts and responsibilities / 唯一事实与职责 | Must not acquire / 不得兼任 |
| --- | --- | --- |
| `ChannelRegistry` + original canonical entry | Source token; immutable publication/entry identity; source revision; desired intent; operation marker; admitted-work tickets and drain; local accepted-result handoff; real resource handles, cancellation/join/cleanup obligations; descriptor publication | No provider generation allocator, owner binding mirror, durable recovery log or parallel runtime table |
| Same entry's `ProviderHandle` | Provider instance identity, legal lifecycle state/transitions and checked generation allocation on every Starting | No channel, tickets, transport tasks, cleanup map or owner state |
| Same entry's `Arc<OwnershipTree>` | Exact visibility binding, owner/lifetime, owner generation, revoke/dispose tombstones | No Gateway resources or implicit provider stop |
| Channel implementation under the source | Protocol-specific I/O and concrete resource operations; returns honest cleanup failures to source | No competing admission/accepted-result authority; no business planning/memory retrieval |
| Resource observation sink, tests only | Receipts at real constructors/open/spawn/request/close/drop/join boundaries, correlated with original entry | No production permission registry, receipt-to-provider lookup or gate based on counters |
| Existing mount table / UI / MCP / CLI | Derived routing or presentation only; all authoritative work returns to original source | No independent channel/provider/disposer catalog |

“runtime authority 拥有 receipts”指它拥有被观察的真实资源边界并提供可关联的测试观察点；**不是在生产创建一张 receipt ledger 决定权限**。The source owns resource handles and cleanup results. Test sinks observe those boundaries; production success depends on actual operations/joins, never receipt counts.

保持一个 `channels` map；operation/work/resource fields 位于其 entry 内。允许模块拆分，不允许第二张 channel→runtime/provider/owner/disposer 表。Per-entry counters and owned JoinHandles are execution accounting, not a second discovery authority. Desired intent is written only by this source; existing health projections remain derived.

## 4. Entry, proof and execution shape / entry、证明与执行形状

中文摘要：lease 是观察，ticket 才是 source 接纳的事实；真实执行必须一直持有原 entry 的不可复制 ticket。调用者取消、任务失联和 transport 错误不能自动清零，更不能写入同名 successor；结果槽只做一次性本地交接，不积累历史或自动重试。

以下是职责草图，名称不是新增 public API。Proposed source-private shape; reuse current declaration/capture/error types where possible:

```text
CanonicalEntry
  immutable: channel, source identity, publication identity/revision,
             descriptor/schema fingerprints, source-minted provider, tree reference
  mutable:   unique lifecycle-operation marker + phase/candidate,
             live non-clone work tickets + accepted-result slots,
             owned forwarder/execution handles + cancellation,
             concrete cleanup obligations and diagnostic residual

ObservationLease = source capture + exact visibility + observed owner generation
AdmittedWork      = original entry + observation lease + unique non-clone ticket
Completion       = Accepted(real transport result) | DiscardedStale(reason)
```

- Caller cannot construct or substitute a provider/tree/ticket. Valid proof originates only at source registration/capture/resolve/admit; equal text IDs or numeric generations are insufficient.
- `resolve` is a cold observation, not admission; all domains are revalidated at use. Unrelated source publications do not stale an unchanged entry.
- A source-owned one-attempt execution retains the ticket from admission through actual transport termination and completion verdict. No await/cancellation gap may strand a newly admitted ticket before execution ownership transfers.
- Caller cancellation drops observation of the result, not ownership of live work. Source retains task/cancel/join ownership in the original entry; no detached transport or child task may outlive its accounting.
- Confirmed-not-dispatched work can settle immediately; a panic/task loss with unconfirmed descendants leaves diagnostic residual, not an optimistic decrement to zero. Settling a known-ended stale/error result releases exactly the original ticket once.
- Local completion slots contain immutable results attached to original entry/ticket and are consumed once; consumed/abandoned slots are released, not accumulated as history. They do not become a durable inbox or replay queue.
- No automatic retry or delivery-store writes are introduced. Every later transport attempt must obtain fresh admission through the source and obey existing transport authorization/telecom constraints.

**Declaration scope / 声明范围：**retain existing Resource + ProviderLifecycle lifecycle-only descriptor, namespace `aleph/gateway/channels`, complete ChannelId name and canonical v1 declaration schema. No Callable/send schema is published. Internal typed `Channel::send` in the test slice proves actual execution/drain, not public operation permission. Future send/ingress needs separately approved operation identity/schema/effect contracts. Tool lease stays Tool-specific.

## 5. Synchronization and linearization / 锁与线性化

### 5.1 One lock order / 唯一锁序

中文摘要：固定 source→tree→provider→entry 锁序；所有同步 callback 只做本地短提交，禁止 await/I/O/外部回调和反向取锁。异步 channel 锁在 authority guard 外获取，实际网络、排空、close/join 都不持 source gate。

**Source lifecycle gate → OwnershipTree guard → ProviderHandle lifecycle guard → entry bookkeeping mutex.** Publication map/desired guards are acquired asynchronously under the source gate, before synchronous callbacks, and released before nested tree/provider/work commits. The channel's async RwLock is acquired only outside authority guards for actual I/O; completion never needs the channel lock.

Callbacks are private synchronous local commits only: no await, I/O, plugins, channel methods, lock re-entry or acquiring source from a tree/provider/work guard. Existing Tool provider→Tool-registry order is unchanged; Gateway must not call Tool registry inside these callbacks. Never obtain an async map lock from a provider/tree callback.

The global source gate protects **short** identity/publication/operation commits, not I/O. Every path that shares this gate, including Legacy start/stop, factory construction, replacement, and `start_all`, must follow the same rule: capture intent under the gate, release it before real channel/provider I/O, then reacquire only for a short validated commit. Retain one unique entry-local operation marker across unlocked phases; another lifecycle request for that entry returns Busy (or explicitly resumes a proven-abandoned cleanup), not a second operation. Different channel I/O can progress independently. Marker is not another provider generation.

A mixed Legacy/Bound progress test is mandatory: block real Legacy channel A I/O while requiring Bound channel B completion, drain, and independent lifecycle progress. The test must fail if any remaining shared gate holder waits on real I/O.

### 5.2 Linearization points / 线性化点

| Boundary / 边界 | Exact point / 具体提交点 |
| --- | --- |
| Registration/capture | Under source gate, publish or read one complete immutable tuple plus desired intent/source revision; no split channel/provider/schema read |
| Activation begin | Install operation marker and enter provider Starting; checked provider generation changes before real resource creation |
| Ready | After real start/setup, validate same entry/marker/generation; provider `with_ready_commit` performs short local activated commit, no async mount work inside callback |
| Admission | Under source→tree→provider→entry guards, validate full source/entry/channel/publication/provider and exact live binding; insert unique ticket and increment original work |
| Ordinary stop | Commit source stop marker/Stopped intent and provider Stopping before releasing gate; no new admission can pass |
| Replacement/unregister | Commit irreversible retirement fence on incumbent under source gate, before waiting for its work; no new admission or old result acceptance after this point |
| Completion acceptance | Under the same authority guards, consume live original ticket and atomically move real result into original local accepted slot; not check-unlock-publish |
| Final replacement/removal | After drain and verified cleanup, under source gate commit entire successor or remove incumbent, intent and checked source revision as one non-suspending cut |

Tree validation compares exact capability + visibility, generation **and** owner/lifetime; same-nonce register replacement is rejected when binding content differs. Identical full binding re-registration is the same authorization fact; explicit bump/revoke is required to force invalidation. Provider guard checks same instance/generation; source compares actual entry/channel identity and full publication fingerprint. None of these domains derives the others or SessionEvent ordering.

## 6. Phased lifecycle, cleanup and publication / 分阶段生命周期与清理

中文摘要：私下准备候选→短锁封闸→解锁排空→真实清理→重验并原子发布。操作跨 await 时仍由原 entry 保管；取消或清理失败保留可诊断残留，只有显式 cleanup 能继续，不允许自动恢复 Ready。入站封闸先于解锁，保留 mount 不等于保留调用权限。

1. **Prepare / 准备：**declaration/channel construction must be cold. Validate candidate and checked publication capacity before touching incumbent. Successor remains private in incumbent operation state until publication; it has no competing map/mount/admission.
2. **Fence / 封闸：**under source gate install operation marker and retirement or ordinary-stop fence. Provider transitions stay legal; cold/disabled stop can be resource-free without inventing a Cold→Stopping transition.
3. **Drain / 排空：**release all gates; wait for actual original work to settle, using register-before-check notification logic. Ordinary stop accepts authentic pre-stop tickets in same-generation Stopping; retirement discards their results but still settles ended work. No channel write lock while waiting on send read guards.
4. **Cleanup / 清理：**cancel/close/join actual forwarders, workers, transport connections and other owned resources outside gates. Resource ownership survives caller/lifecycle-future cancellation: store operation-owned handles and candidate before awaiting. Explicit cleanup retry first proves prior driver ended/joined; it cannot clear a still-running operation marker.
5. **Commit / 提交：**revalidate original entry/marker, acquire publication locks before first write, then publish without await. Ready requires setup success; Stopped/removal/successor require cleanup success. Stop failure retains incumbent identity, mount metadata and honest residual, never restores Ready automatically.

**Ingress during cleanup / 清理期入站：**retirement/stop must gate dispatch before releasing the source gate. A retained mount is not permission to invoke raw handlers. In this unmounted slice Bound mounts use a source-owned deny-only wrapper and Bound forwarders never publish into legacy inbound consumers. Held wrappers must deny even when called after unmount. Existing legacy routing is not redesigned here.

**Revision exhaustion / revision 耗尽：**preflight before destructive retirement; final checked allocation under gate is authoritative. Another entry may consume remaining capacity during unlocked cleanup. Then retain the now-stopped incumbent and report PublicationExhausted, with no successor/removal or wrap. Cleanup success is not rolled back or relabelled Ready. A physically closed resource/mount is not resurrected to imitate an earlier snapshot. Map membership/revision stays honest; caller sees changed lifecycle state plus explicit failed publication.

**Residual / 残留：**report original entry, phase, active/unconfirmed work, unjoined tasks, potentially live transport and retained mount. Diagnostic fields do not allocate authority. Timeout returns Busy/residual, never “drained”; explicit stop/cleanup is the only retry policy. Revoke/dispose do not silently clean a shared provider. Contract-only replacement reuses the retirement protocol and leaves the new publication stopped unless explicitly activated.

## 7. Stale-result policy / 陈旧结果矩阵

中文摘要：普通 stop 允许真实的同代 pre-stop ticket 完成；retirement/revoke/dispose 先发生则拒收旧结果，但结清原工作。先接受的结果不追溯撤销。成功替换前必须真实排空；替换后可晚到的是调用方观察，不是未记账的旧 transport。

| Event / 事件 | Admission / 准入 | Completion / 完成 | Resources / 资源 |
| --- | --- | --- | --- |
| Ordinary stop first | Reject | Genuine pre-stop ticket, same generation, non-retiring: may accept while Stopping | Drain then close/join; only then Stopped |
| Retirement first | Reject | Discard; settle only original ended work | Successor waits for full drain/cleanup |
| Provider restart/different instance | Old proof rejects even if numbers match | Old proof stale | Restart requires old cleanup; Starting increments generation |
| Owner bump or same-nonce owner/lifetime replacement | Reject old full binding | Reject if it linearized before acceptance | No automatic whole-provider shutdown |
| Revoke/dispose first | Fail closed; reads never restore binding | Discard and settle ended original work | Separate explicit lifecycle cleanup only |
| Completion first, later revoke/retire | No new authority implied | Already accepted fact remains accepted, not retroactively undone | Downstream delivery still obeys its own visibility/auth checks |
| Transport error | Real admitted attempt | Accepted(Err), not fake success/stale or automatic retry | Settle after actual end |
| Timeout/caller cancel/task loss | No hidden extra admission | Wait for end, or preserve unconfirmed residual | No “Future dropped ⇒ resource closed” inference |
| Old caller receives result after successor publication | Old lease rejects | Cannot mutate successor slot/count/mount | Full drain excludes genuinely running old transport after successful publication |

A remote side effect may already have happened before stale completion is discarded. 本地接受 fence 不是外部 exactly-once、撤回或安全重放。Do not forge duplicate tickets to manufacture impossible post-drain execution; test delayed result observation separately from still-live work.

## 8. Real resource evidence / 真实资源回执

每实例测试 sink 注入实际 source/adapter 边界，不使用全局 counters。记录 attempts、success、active、close/drop、cancel/join，失败尝试也计数。Receipts observe operations, never authorize them. Metadata/Arc/ordinary broadcast allocations are explicitly outside provider-runtime resource counts.

| Resource / 资源 | Required real boundary / 实际取证点 | Coverage limit / 限制 |
| --- | --- | --- |
| HTTP/token client, auth/token requests | Actual builder/constructor and request poll/peer receipt separately | Client count is not proof of no authentication |
| Worker/forwarder/thread/timer | Actual spawn/interval creation, cancellation and awaited join | Drop of JoinHandle is not join; absent thread is N/A |
| Listener/connection/websocket | Bind/connect/handshake attempts, actual close/end | TCP fixture does not prove websocket cleanup |
| Database/provider cache | Real open/init, close/drop and worker join if applicable | Immutable declaration snapshots are not provider caches |
| Work and result | Ticket insertion, actual Channel transport entry/peer effect, acceptance/discard, original drain release | Calling validate alone is not proof of blocked effect |

首个 vertical slice 选择**现有 Generic WebhookChannel**，通过 loopback peer 和测试 Bound composition 验证，不挂生产。其 `new()` 当前 eager client construction 必须真正延后到 activation；`with_client` 的预建 client 不具 cold-construction 证明，不能作为矩阵基线。默认冷态 Webhook send-before-start 应明确返回 NotConnected，而不是在 send 中隐式建 client；这是一个需单独覆盖旧调用者的兼容收紧，不宣称旧冷态 send 行为不变。Default cold send must reject before activation; this specific compatibility tightening requires caller/regression audit. Explicit prebuilt-client legacy injection remains outside the cold proof. Second shape is a test-only polling/socket fixture with actual worker/timer/TCP/cache/SQLite boundaries; it cannot establish that all production adapters are cold.

## 9. Discovered / Cold / Disabled matrix / 冷态矩阵

所有 zero 为每类适用 provider-runtime attempts/active 的增量零；不得只断言 ProviderState 不变。All cells require repeated reads, negative cases and actual positive controls.

| Surface / 操作 | Discovered | Cold | Disabled |
| --- | --- | --- | --- |
| Construct/register declaration | Lightweight only; zero | Same / 同左 | Same; boot does not start |
| Describe/list/project/source snapshot | Lifecycle declaration only; zero | Same | May show disabled; zero |
| Capture | Observation only; zero | Same | Same |
| Resolve/admit/old proof use | NotReady; zero tickets/effects | Same | Same |
| Unknown ID/wrong scope/repeated failed reads | No creation/auth/network; zero | Same | Same |
| Treat lifecycle descriptor as send Callable | Not provided; zero | Same | Same |
| Explicit activation | Starting may allocate; positive control | Same | Explicit management only; never triggered by reads |

Ready internal work still requires every admission predicate and existing authorization; descriptor never becomes Callable. Failed may contain real residual. Stop requires verified zero active resources and completed applicable joins; reads of residual do not clean or start anything. Resource classes not actually exercised are labelled **N/A / 未证明**, not passed.

## 10. Many-channel migration / 多通道迁移

中文摘要：普查所有实现、插件注册与直接 boot 入口；每通道记录真实 constructor/start/stop/子任务/共享资源和准入旁路。先闭环非飞书实例，再逐个 opt-in；未经审计的通道仍为 Legacy，不靠 implements Channel 自动授予新保证。其他子系统以后各守自己的 source owner，不包装成 channel。

- Inventory is derived from `src/gateway/interfaces/mod.rs`, actual `impl Channel/ChannelFactory`, plugin registrations, `create_channel_from_config`, direct boot registrations and plugin transports. A static list of names is not complete evidence; every discovered implementation needs a row or an explicit exclusion.
- Each row records constructor/start/stop/resource owners, workers/descendants and join ability, webhook/forwarder routes, outbound optional/retry/telecom/directory paths, shared-runtime dependencies, current Legacy/Bound disposition and unproven receipts. Inventory is review evidence, not a production registration map.
- First prove real Generic Webhook source-to-result-to-cleanup without production opt-in. Then prove replacement/removal; then close every Bound compatibility bypass centrally. A second transport shape tests genericity; no Feishu condition or channel-type switch is permitted in admission/drain.
- Migrate concrete channels one by one only after constructor/resource and authorization audits; unreviewed ones remain explicitly Legacy and receive no Bound guarantee. Boot/config entry points must converge on this same authority in a later production migration, not register once for runtime and again for Capability.
- Shared providers and adapters without provable join/cleanup remain blocked until their owner contracts exist. Terminal/memory graph/sandbox require their own source owners, not channel wrappers or a universal Gateway supervisor.

## 11. Exclusions and gate / 排除项与验收

**Not authorized here / 本阶段明确排除：**universal scheduler; durable admission/recovery; EffectClaim producer; Safe Replay; external exactly-once; generalized envelope; new CapabilityKind; Feishu implementation/R2; production Bound opt-in/mount; Zahir route/backend; public send/ingress operation; Tool lease rewrite; SessionEventStore changes; Harness changes; process isolation or executable-code unloading.

设计交付只创建本增补和计划，不修改 Rust、旧文档或参考状态。Implementation must produce deterministic RED→GREEN tests at actual boundaries, nonzero test counts, independent whole-diff review and honest per-resource limits. Even those results require a **separate R1 gate decision**; they never automatically open Feishu/R2 or production mount.
