# Stage H Readiness Plan: Provider-Neutral Lazy Activation and Activation Ownership
# Stage H 就绪性计划：Provider-Neutral Lazy Activation 与 Activation Ownership

## Goal / 目标

Determine whether Aleph can make provider activation lazy at the existing production boundary while preserving the North Star: every externally composable capability is a Capability, and disabled capabilities do not initialize their runtime resources on process startup.

判断 Aleph 是否能在既有 production boundary 上实现 lazy provider activation，同时保持 North Star：一切 externally composable 能力都是 Capability，关闭的能力不会在进程启动时初始化自己的 runtime resources。

Stage H begins as a readiness/design gate. It may produce a narrowly scoped implementation only after the source census proves that the existing Extension/RuntimeOwner/ProviderHandle/registry authorities are sufficient. It must end NO-GO with an architecture decision request if satisfying the contract requires a second authority.

Stage H 首先是 readiness/design gate。只有 source census 证明既有 Extension/RuntimeOwner/ProviderHandle/registry authority 足够时，才能进行最小实现；如果 contract 要求第二套 authority，必须以架构决策请求结束并停止实现。

## Current baseline / 当前基线

Stage F established real provider state, generation, admission, replacement fencing, and exact Extism instance ownership through the existing Extension lifecycle. Stage G established the production provider-neutral `ProjectionHost` mount using the startup canonical `ToolHandlerRegistry` and one `Arc<OwnershipTree>`, and was merged to `main` at `f43aa65c8`.

The unresolved gap is lazy activation at the canonical provider boundary. Production currently constructs `ToolHandlerRegistry::new()` with a Ready provider lifecycle; `ToolHandlerRegistry::new_with_provider` and `ProviderHandle::subscribe_changes()` are design/entry points without a production caller. Existing Extension activation already has a real `RuntimeOwner` and provider handle, so the first question is whether that owner can drive lazy activation without creating a parallel lifecycle.

Stage F 证明了既有 Extension lifecycle 中真实的 provider state、generation、admission、replacement fencing 与 exact Extism instance ownership。Stage G 证明了 production 使用 startup canonical `ToolHandlerRegistry` 与唯一 `Arc<OwnershipTree>` 挂载 provider-neutral `ProjectionHost`，并已在 `main` 的 `f43aa65c8` 完成合并。

尚未闭合的缺口是 canonical provider boundary 的 lazy activation。Production 当前使用 `ToolHandlerRegistry::new()`，其 provider lifecycle 为 Ready；`ToolHandlerRegistry::new_with_provider` 与 `ProviderHandle::subscribe_changes()` 仍是 design/entry point，没有 production caller。既有 Extension activation 已经拥有真实 `RuntimeOwner` 与 provider handle，因此第一问题是：能否由该 owner 驱动 lazy activation，而不建立并行 lifecycle。

## Architecture and authority contract / 架构与 authority contract

| Concern | Existing authority | Stage H rule |
|---|---|---|
| Discovery and durable enabled/disabled | `plugin_state.rs` and `ExtensionManager` | Reuse; do not add a second desired-state source |
| Runtime ownership and exact cleanup | `RuntimeOwner`, `EffectScope`, loader disposer | Reuse; retain residual failures and exact identity semantics |
| Provider state/generation/admission | `ProviderHandle` and `ProviderLifecycle` | Reuse; transitions and permits remain the only call gate |
| Canonical callable publication | `ToolHandlerRegistry` | Reuse; no mirrored catalog or registry |
| Owner/revoke/dispose facts | `OwnershipTree` | Reuse; no provider-local universal tree |
| Projection and notification | `ProjectionHost` and `ZahirFacade` | Derived read-side only; never starts or owns provider lifecycle |
| Channel association | `ChannelRegistry` | Keep channel-local; do not promote to universal backend |

The plan must preserve the existing dependency direction and the R1–R10 redlines. `ProviderLifecycle` may describe and gate a provider, but it must not become a new durable enabled/disabled authority. ProjectionHost may observe lifecycle/publication changes, but it must not trigger activation from read-side access.

## Approaches / 方案

### Approach A — Reuse the existing runtime owner and provider handle (recommended)

Trace the existing `RuntimeOwner` activation path to the provider handle and canonical Tool publication. Add only the smallest missing call or test seam at the owner boundary, if the current types already express it. Keep registry and ProjectionHost derived from the same canonical source.

**Trade-off:** this keeps authority correct and aligns with Stage F/G, but may require a narrow change at the Extension-to-registry boundary and careful single-flight tests.

### Approach B — Add a thin explicit activation request at the canonical registry boundary

Expose an explicit request that delegates immediately to the existing runtime owner, while keeping all state, generation, receipt, and cleanup in the existing owner. The request must reject or hide non-Ready surfaces and must not start from `describe`, `resolve`, `project`, or subscription.

**Trade-off:** it can make the contract easier to call, but risks becoming a second activation API or hiding a second owner. Use only if Approach A cannot express the public boundary without an adapter that remains purely delegating.

### Approach C — Add a generic Capability activation coordinator or provider catalog (rejected)

A new manager could own provider descriptors, activation state, generation, and cleanup across Tool, Gateway, Terminal, Memory, and Sandbox.

**Why rejected:** it duplicates `ExtensionManager`, `RuntimeOwner`, `ProviderHandle`, `ToolHandlerRegistry`, `OwnershipTree`, and channel-local authorities. It would violate the current architecture before the production contracts exist.

方案 A 是推荐方案；方案 B 仅在现有 owner boundary 无法表达 explicit activation 时作为纯 delegation 备选；方案 C 会复制现有 authority，因此拒绝。

## Work phases / 工作阶段

### Phase 0 — Preflight and source census

- Verify actual git root, `HEAD`, branch, worktree, remote relationship, and current prompt/plan.
- Read `AGENTS.md`, redlines, Feature Locator §3.5d and Stage F/G records, architecture/provider/plugin references, and the exact source files.
- Trace every production caller of `ToolHandlerRegistry::new()` and `new_with_provider`.
- Trace `RuntimeOwner` construction, activation, provider transition, canonical publication, replacement, stop, disposer, and residual paths.
- Trace `ProviderHandle::subscribe_changes()` and determine whether it is an observation stream or an activation source.
- Record whether `ProjectionHost`, `ZahirFacade`, `ChannelRegistry`, `PtyManager`, or `MemoryExtensionRegistry` would become a second authority under each approach.

**Gate:** if no existing owner can drive activation, write the NO-GO decision request and stop before implementation.

### Phase 1 — Design and RED contract

- Present the approaches in a separate user-facing message and obtain a choice.
- Write a bilingual design at `docs/superpowers/specs/YYYY-MM-DD-gateway-stage-h-provider-lazy-activation-design.md`.
- Obtain explicit design approval.
- Add the smallest RED evidence in the existing Extension lifecycle test module, using a real WASM fixture and existing probes. The RED must fail for the missing behavior, not for a guessed API or unrelated fixture setup.
- Keep existing Stage E/F/G helpers and fresh-tree compatibility helpers unchanged unless the shared authority contract requires a narrow wrapper.

**Gate:** no production implementation, dependency installation, scaffolding, or implementation skill before design approval.

### Phase 2 — Minimal authority-preserving implementation

Only if Phase 0 and the approved design pass:

- Connect explicit activation to the existing runtime owner and `ProviderHandle`/`ProviderLifecycle` transition path.
- Preserve serialized/single-flight activation for one provider.
- Keep canonical registry publication and ProjectionHost view derived from existing sources.
- Hide or reject non-Ready `describe`/`resolve`/callable lease surfaces.
- Preserve failure residuals and exact disposer/identity behavior.
- Avoid adding a universal provider manager or modifying concrete channel lifecycle.

If the change would require a second lifecycle, stop and convert the remaining work into an architecture decision request.

### Phase 3 — RED→GREEN evidence

At minimum, prove with real observations:

1. cold discovery/read-side list, descriptor, `describe`, `project`, and subscription do not read guest bytes, create instances, transition to Starting, or initialize provider resources;
2. one explicit owner activation reaches Ready and creates exactly one current generation/publication;
3. concurrent activation is single-flight or otherwise serialized by an existing owner;
4. failed read/build/start/publication/dispose preserves the existing Failed/residual state;
5. non-Ready surfaces fail closed and do not return a usable lease or callable route;
6. stop/replacement fences stale provider, generation, binding, facade, and exact receipt;
7. explicit restart publishes a successor through the same canonical registry and ProjectionHost.

Use existing Stage E/F/G and B3 evidence rather than duplicating every established assertion. A single WASM fixture proves only the bounded in-process path.

### Phase 4 — Documentation and verification

- Update the bilingual run record under `docs/superpowers/runs/` with actual commands, memory gates, counts, warnings, residuals, proof limits, reviewer result, and deferred work.
- Update `docs/reference/FEATURE_LOCATOR.md` only at the applicable Capability/Stage H boundary; preserve historical Stage E/F/G evidence verbatim.
- Run each Cargo command only after a separate `python qa/browser_live/check_memory.py` returns exit 0 and `ALLOW`.
- Run focused Stage H tests serially three times with non-zero counts.
- Run justified narrow Stage E/F/G, B3, ProjectionHost, capability, gateway, and WASM regressions, then check, clippy, fmt, and diff check.
- Dispatch one independent read-only reviewer over source, tests, docs, and the complete diff.

## Suggested source/test locations / 建议源码与测试位置

- `src/capability/provider.rs`: provider transition, generation, admission, subscription semantics;
- `src/tools/registry.rs`: `new`, `new_with_provider`, canonical publication and callable surface;
- `src/capability/projection_host.rs`: read-side host composition and notification behavior;
- `src/capability/zahir_facade.rs`: derived Tool projection and lease/resolve boundary;
- `src/extension/plugin_state.rs`: durable discovery and enabled/disabled authority;
- `src/extension/lifecycle.rs`: existing real WASM activation, replacement, stop and residual tests;
- `src/extension/runtime_owner.rs`: runtime owner task, identity, receipt and admission ownership;
- `src/extension/loader.rs`: exact WASM identity/disposer authority;
- `src/bin/aleph-server/commands/start/mod.rs` and startup builder: production canonical registry/host composition;
- `src/gateway/channel_registry.rs`: confirm channel-local authority remains separate.

Prefer extending the existing lifecycle test module near the Stage F/G tests. Do not create a new generic provider test harness that duplicates ownership or resource accounting.

## Acceptance criteria / 验收标准

The plan is complete only if:

- actual production callers and owners are source-backed;
- the selected approach reuses one existing activation and cleanup authority;
- cold read-side access is proven resource-free within the stated observable boundary;
- explicit activation, failure, non-Ready fail-closed behavior, replacement, stop, and restart are proven with real non-zero tests;
- no second registry, catalog, owner map, generation map, runtime table, lifecycle, or cleanup authority is introduced;
- no concrete Feishu/Gateway channel, Terminal, Memory Graph, Sandbox, or universal dynamic-loading claim is added;
- docs state exact proof limits and all residuals;
- required memory gates and verification commands are recorded;
- an independent reviewer approves the bounded result or its exact unavailability is recorded without claiming approval.

## Deferred boundaries / 延后边界

Feishu→Zahir integration, concrete Gateway channel dynamic loading, Terminal, Memory Graph, Sandbox, remaining capability kinds, EffectClaim producer integration, automatic Safe Replay, universal durable scheduling, external-effect exactly-once, native unloading, process isolation, early startup error drain, engine/JIT reclamation, OS mappings, thread shutdown, and RSS claims remain separate architecture gates.

## Final report questions / 最终报告问题

1. Which existing runtime owner triggers activation?
2. What are the exact read/create/transition effects of cold discovery, explicit activation, replacement, stop, and restart?
3. Which authority owns enabled/disabled, provider state/generation/admission, publication, ownership, and cleanup?
4. How is single-flight activation proven?
5. What stale handles and exact receipts remain alive until which references are released?
6. What residuals survive failure, and how are they represented?
7. Which resource and provider claims remain unproven or deferred?
8. Were any concrete channels or new authorities added?
9. Which commands, memory gates, tests, and reviewer evidence passed?
10. Which destructive, external, merge, push, or publication actions were not performed?
