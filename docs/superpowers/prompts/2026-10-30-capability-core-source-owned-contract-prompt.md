# Next Session Prompt: Generic Gateway Source-Owned Capability Slice
# 下一会话 Prompt：Generic Gateway source-owned Capability slice

You are continuing Aleph Capability Core work in `/Volumes/TBU4/Workspace/Aleph`.

你将在 `/Volumes/TBU4/Workspace/Aleph` 继续 Aleph Capability Core 工作。

## Read first / 首先阅读

1. `AGENTS.md`
2. `docs/reference/REDLINES.md`
3. `docs/reference/ARCHITECTURE.md`
4. `docs/reference/GATEWAY.md`
5. `docs/reference/TOOL_SYSTEM.md`
6. `docs/reference/FEATURE_LOCATOR.md`
7. `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-design.md`
8. `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-claim-census.md`
9. `docs/superpowers/plans/2026-10-30-capability-core-source-owned-contract-plan.md`
10. `src/capability/{descriptor,provider,facade,backend,ownership,zahir_facade}.rs`
11. `src/gateway/channel_registry.rs`
12. `src/gateway/channel_registry/binding.rs`

## Mission / 任务

Implement only the approved, bounded Generic Gateway source-owned read/lease slice. Preserve the existing `ChannelRegistry` as the sole Gateway association, lifecycle, source revision, replacement, stop/drain, and webhook publication authority. The Capability layer must remain a derived read/projection surface.

只实现已批准且受限的 Generic Gateway source-owned read/lease slice。保留现有 `ChannelRegistry` 作为唯一 Gateway association、lifecycle、source revision、replacement、stop/drain 和 webhook publication authority。Capability 层必须继续是 derived read/projection surface。

The first slice covers:

首个 slice 包含：

- `describe`
- `list`
- `project`
- source-owned `resolve`/lease capture and validation

It explicitly excludes Gateway `subscribe`, new observer wiring, durable recovery, Feishu → Zahir routing, Feishu production mounting, Terminal/Memory/Sandbox migration, new `CapabilityKind`, EffectClaim producers, Safe Replay, external exactly-once, universal scheduling, native unload, process isolation, RSS reduction, and `src/harness/**`.

明确不包含 Gateway `subscribe`、新的 observer wiring、durable recovery、Feishu → Zahir routing、Feishu 生产挂载、Terminal/Memory/Sandbox migration、新 `CapabilityKind`、EffectClaim producer、Safe Replay、external exactly-once、universal scheduler、native unload、process isolation、RSS reduction 和 `src/harness/**`。

## Settled decisions / 已确定决策

- Approach: source-owned canonical binding + derived Capability projection.
- Generic Gateway owner: `OwnerRef::Runtime`.
- Generic Gateway lifetime: `LifetimeScope::Runtime`.
- Visibility: supplied by `ChannelLifecycleContract`; never inferred by the projection.
- Coherent read: anchor at the `ChannelRegistry` source cut, then validate provider and owner evidence. If source revision or entry identity changes, use only a bounded source-defined retry or fail closed.
- `source_revision` and `descriptor_revision` are separate facts and are never interchangeable. Failed publication advances neither.
- A live broadcast is notification-only and is not a durable recovery source.
- Real runtime effects remain outside pure read operations: no token refresh, HTTP, listener bind, websocket connection, task spawn, process spawn, database open, cache activation, or extension execution.

- 方案：source-owned canonical binding + derived Capability projection。
- Generic Gateway owner：`OwnerRef::Runtime`。
- Generic Gateway lifetime：`LifetimeScope::Runtime`。
- visibility：由 `ChannelLifecycleContract` 提供，projection 不得自行推导。
- coherent read：以 `ChannelRegistry` source cut 为锚，再验证 provider 与 owner evidence；source revision 或 entry identity 变化时，只能进行受限 source-defined retry 或 fail closed。
- `source_revision` 与 `descriptor_revision` 是独立事实，永远不可互换。publication 失败时两者都不推进。
- live broadcast 仅是 notification，不是 durable recovery source。
- 真实 runtime effect 必须位于纯 read operation 之外：不得 token refresh、HTTP、listener bind、websocket connection、task spawn、process spawn、database open、cache activation 或 extension execution。

## Execution protocol / 执行协议

1. Confirm the worktree and read the plan before editing.
2. Work task-by-task in the order of the implementation plan.
3. Write the failing focused test before each implementation step.
4. Run the focused test, then package checks, then the required broader checks.
5. Keep one source of truth per fact; do not add a parallel map or catalog.
6. If the current `BackendLease` shape cannot represent a Gateway source-owned proof without creating a dependency cycle or changing the Tool authority, stop and report the architectural blocker instead of introducing a generic workaround.
7. Do not silently expand the first slice to `ZahirFacade` mount, Gateway subscription, Feishu, or other providers.
8. Before any production change, ensure the implementation plan has been explicitly reviewed and accepted in the current session.

1. 确认 worktree 状态，并先阅读 plan 再编辑。
2. 按 implementation plan 的顺序逐个 task 执行。
3. 每个 implementation step 前先写 failing focused test。
4. 先运行 focused test，再运行 package checks，最后运行要求的 broader checks。
5. 每个事实只保留一个 source of truth；不得添加平行 map 或 catalog。
6. 如果当前 `BackendLease` 无法在不制造依赖 cycle 或改变 Tool authority 的前提下表达 Gateway source-owned proof，应停止并报告 architecture blocker，不得引入通用 workaround。
7. 不得静默扩大首个 slice 到 `ZahirFacade` mount、Gateway subscription、Feishu 或其他 provider。
8. 任何 production change 前，确认 implementation plan 已在当前 session 被明确审阅并接受。

## Acceptance / 验收

The slice is complete only when:

只有满足以下条件，slice 才算完成：

- cold/disabled Generic Gateway reads return truthful descriptors without starting runtime effects;
- `describe`, `list`, and `project` share the source-backed snapshot and no second catalog exists;
- source-owned resolve captures and validates entry identity, source revision, descriptor revision, provider generation, owner identity/generation/lifetime, and visibility;
- replacement, unregister, owner transfer, provider stop, and late completion reject stale leases fail-closed;
- checked revision exhaustion preserves the incumbent and performs no destructive partial mutation;
- focused tests, `cargo fmt --check`, `cargo check -p alephcore --tests`, and relevant `cargo test` commands pass;
- the final diff contains no Feishu mount, new CapabilityKind, second registry, harness change, or unrelated provider migration;
- reference documents are updated only from verified source/test evidence and retain all deferred boundaries.

- cold/disabled Generic Gateway read 返回真实 descriptor，且不启动 runtime effect；
- `describe`、`list` 和 `project` 共享 source-backed snapshot，不存在第二 catalog；
- source-owned resolve 捕获并验证 entry identity、source revision、descriptor revision、provider generation、owner identity/generation/lifetime 与 visibility；
- replacement、unregister、owner transfer、provider stop 和 late completion 都能 fail-closed 拒绝 stale lease；
- checked revision exhaustion 保留 incumbent，不产生 destructive partial mutation；
- focused tests、`cargo fmt --check`、`cargo check -p alephcore --tests` 和相关 `cargo test` 通过；
- final diff 不包含 Feishu mount、新 CapabilityKind、第二 registry、harness change 或无关 provider migration；
- reference 文档只根据已验证的 source/test evidence 更新，并保留所有 deferred boundary。

## Stop conditions / 停止条件

Stop and ask for architecture review if any implementation requires:

如遇到以下任一情况，停止并请求 architecture review：

- a second Gateway authority or descriptor catalog;
- a new capability kind or a new universal runtime table;
- a cross-authority transaction that changes existing lock ownership;
- making `ProviderHandle` perform runtime effects;
- changing `Zahir` or `BackendLease` in a way that makes Core depend on Gateway;
- using live broadcast as replay or durable recovery;
- proving external delivery, exactly-once, native unload, RSS reduction, or process isolation.

- 第二个 Gateway authority 或 descriptor catalog；
- 新 capability kind 或新的 universal runtime table；
- 改变现有 lock ownership 的 cross-authority transaction；
- 让 `ProviderHandle` 执行 runtime effect；
- 以使 Core 依赖 Gateway 的方式修改 `Zahir` 或 `BackendLease`；
- 将 live broadcast 当作 replay 或 durable recovery；
- 声称证明 external delivery、exactly-once、native unload、RSS reduction 或 process isolation。
