# Capability Core provider activation design / Capability Core 提供者激活设计

- Date / 日期: 2026-11-04
- Status / 状态: Draft for the next session — plan and restart prompt written; **not implemented** / 供下一会话使用——计划与重启提示已写；**尚未实施**
- Scope / 范围: Make a second capability kind first-class in the Capability Core: kind-neutral provider visibility plus explicit on-demand activation, with Terminal as the first proof instance
- Prior evidence / 前序证据: [2026-11-03 capability tool mount evidence run](../runs/2026-11-03-capability-tool-mount-evidence-run.md) · [2026-11-03 Memory source-owned boundary run](../runs/2026-11-03-memory-source-owned-boundary-run.md) · [2026-11-03 Memory source-owned validation closure run](../runs/2026-11-03-memory-source-owned-validation-closure-run.md)

## Intent / 意图

The north star is **"Everything externally composable is a Capability"**: anything an Agent, the Runtime or another system must be able to discover, call, replace, subscribe to or manage is a Capability. Aleph is to become a light Capability Core plus a set of on-demand, stoppable, replaceable providers — Gateway's social channels, the terminal, the memory nebula, sandbox, browser, computer-use — so that a disabled capability costs no background work.

This slice answers exactly one question about that direction: **why can the Core not see or start a provider today, and what is the smallest kind-neutral contract that fixes it?** The answer must not be shaped by one subsystem. The Core must learn to describe, resolve and *ask to start* any provider family, and one existing family must prove it end to end with real resources.

本切片只回答一个问题：**为什么 Core 今天看不见、也起不动一个 provider？** 最小且 kind-neutral 的契约是什么？答案不能由某一个子系统塑造：Core 必须能描述、解析并**请求启动**任意 provider 家族，并由一个既有家族用真实资源端到端证明。

## Current evidence / 当前证据

The kind-neutral lifecycle contract **already exists** and is already driven by four structurally different families — but the Core does not *serve* it.

| Surface / 面 | Today / 现状 | Consequence / 后果 |
|---|---|---|
| `CapabilityBackend` trait (`src/capability/backend.rs`) | `lookup` / `enumerate` / `snapshot` — signatures are already kind-agnostic | The Core's backend contract can host any kind; nothing stops a second adapter |
| `ToolBackendAdapter` (`src/capability/backend.rs:37-138`) | `in_scope` returns `false` for any `kind != CapabilityKind::Tool` (`:49`); `to_descriptor` hardcodes `kind: CapabilityKind::Tool` (`:138`) | The only adapter is Tool-shaped |
| Backend coverage (`src/capability/backend.rs:26`) | "eight kinds are deferred and non-invocable — no backend implements them and none is mounted" | Every kind except Tool is non-invocable by construction |
| `ZahirFacade` (`src/capability/zahir_facade.rs:100-105`) | `{ adapter: ToolBackendAdapter, tree, memory: Option<Arc<MemoryRuntime>> }` | One hardcoded provider special case (Memory); no generic provider path |
| `ZahirFacade::resolve_with_capture` (`:410-460`) | Rejects any namespace `!= TOOL_NAMESPACE` (`:411-413`); lease carries `BackendSourceEvidence::Tool { provider_generation, canonical_entry }` | Only Tool capabilities can be resolved into a lease |
| `ProviderLifecycle` / `ProviderHandle` (`src/capability/provider.rs`) | Kind-neutral: `ProviderId`, `ProviderTransport`, `ProviderState`, `ProviderLifecycle::new/ready`, `transition`, `acquire_call(generation)`, `subscribe_changes` | The lifecycle contract exists and is generic; families must expose it |
| Families already using it | WASM plugins `src/extension/runtime_owner.rs:156-161`; Terminal `src/gateway/pty/manager.rs:318-328`; Gateway channels `src/gateway/channel_registry/capability.rs:304`; Feishu webhook server `src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:301` | Four different owners already drive the same core contract |
| `CapabilityKind::Resource` | Used in production for Memory (`src/memory/runtime/mod.rs:326`) and for Gateway channels (`src/gateway/channel_registry.rs:873`, `src/gateway/channel_registry/binding.rs:1343`) | "Heavy, locally-owned, startable provider" already has a kind — no tenth kind is needed |
| `Zahir` trait (`src/capability/facade.rs:315-320`) | `describe` / `resolve` / `validate_lease` / `subscribe` / `project` | No way to *request activation*; `resolve` is documented as never starting anything |
| `OwnershipTree` | `register_if_absent` / `rotate_capability` / `generation` — ownership generation per capability id | The Core already owns capability-level fencing; provider-level fencing lives in `ProviderLifecycle` |

**Gap statement.** Families comply with the Core's lifecycle contract, but the Core only *serves* Tools plus one hardcoded provider. Therefore a provider family cannot be discovered through `describe`, cannot be resolved into a lease through `resolve`, and cannot be started at all through the Core. Today the only way to start Terminal is to call `PtyManager::activate` from Gateway code that already knows about Terminal — exactly the shape the north star wants to remove.

**缺口陈述。** 各家族已经遵守 Core 的 lifecycle 契约，但 Core 只*提供* Tool 加一个硬编码 provider。因此 provider 家族无法经 `describe` 被发现、无法经 `resolve` 拿到 lease、也无法经 Core 被启动。今天启动 Terminal 的唯一路径是 Gateway 代码直接调用 `PtyManager::activate`——正是终极目标要消除的形状。

## Goals / 目标

1. **One Core-owned mechanism** describes, resolves and activates provider-shaped capabilities of any family. No per-family branch inside the Core.
2. `resolve` still never starts anything. Activation is an explicit, separate request.
3. Activation is **owner-performed** (R1: the Core never performs platform work) and **fail-closed**: a failed activation leaves the capability unavailable and never yields a stale lease.
4. A provider lease fails closed when the provider is stopped or replaced (generation fencing on both the ownership side and the provider side).
5. **One statement per fact:** Memory stops being a hardcoded facade field and is served by the same generic path, so "a Resource capability" means exactly one thing in the Core.
6. The contract stays kind-neutral: it is proven with Terminal, which is structurally unlike both plugins and channels.

## Non-goals / 非目标

- **Not** "the code is not linked into the binary". In-process laziness is not binary slimness; that needs a Bridge/process split (R1) and is a later slice.
- No universal all-writer stop, no native unload, no RSS or process-isolation claim.
- No EffectClaim producer, no external-effect exactly-once, no automatic Safe Replay, no universal durable scheduler.
- No channel / Feishu / MCP / browser / sandbox / computer-use registration in this slice — Terminal is the only new instance.
- No second registry for tools, no new public test-only API, no change to `OwnershipTree` semantics.
- No rewrite of a family's own state machine: Memory keeps `MemoryState`, Terminal keeps `DriverState`. The Core observes a `ProviderHandle`.

## Anti-goals / 反目标

A slice that technically "works" but fails these is a failure:

- the Core growing a `terminal`/`memory` special case (判据 #1: one fact, two statements);
- a capability that can be described but neither resolved nor activated;
- activation happening implicitly as a side effect of `resolve`;
- the Core performing the effect itself instead of asking the owner (R1).

## Approaches considered / 考虑过的方案

**A. Core-owned provider registry + provider-backed backend + explicit activation request (chosen).**
The Core gains a registry of *providers* (not callables): each entry carries a `CapabilityDescriptor`, a `ProviderHandle`, an owner and an activation hook. A second adapter implements the existing kind-agnostic `CapabilityBackend` trait, so `describe`/`enumerate`/`snapshot` serve provider descriptors. The `Zahir` trait gains an explicit `activate` that asks the owner to start and returns a fresh, fenced lease.
*Trade-off:* the Core gains one registry and one adapter — but it is a registry of a genuinely different shape (providers vs callables), and the alternative (a registry of callables) already exists as `ToolHandlerRegistry`.

**B. Make the facade generic over "capability sources"** (each family implements a `CapabilitySource` trait).
*Rejected:* each family would then own descriptor and lease semantics, so the lease contract and the fencing rule would be duplicated per family — the exact "same fact, two statements" shape the Core exists to prevent. The Core must own the contract; owners own the effects.

**C. Keep the hardcoded Memory case and add one hardcoded Terminal case.**
*Rejected:* the Core's shape would be decided by the second instance, and `Resource` would mean two different things depending on which field answered. This is also the mistake the user corrected on 2026-10-26: the canonical contract must serve all families, with any one family being only an instance.

**Chosen: A**, with the discipline that the contract is written against the *family* shape (descriptor + lifecycle handle + activation hook) and that Terminal contributes nothing to the contract's vocabulary.

## The contract / 契约（proposed shape；Task 1 定名与定形）

Core-side, in `src/capability/`:

1. **`ProviderRegistry`** — one authority for provider-shaped capabilities. Entry = `{ descriptor, provider: ProviderHandle, owner: OwnerRef, activation: ProviderActivator, visibility }`. Operations: register / update descriptor / revoke / lookup / enumerate-by-scope. It is **not** a second tool registry: tools stay in `ToolHandlerRegistry`.
2. **`ProviderBackendAdapter`** — implements the existing `CapabilityBackend` trait (`lookup` / `enumerate` / `snapshot`) over the registry, honouring `Scope { kind, namespace, facet, visibility }`.
3. **`ZahirFacade` composition** — `ToolBackendAdapter` (callables) + `ProviderBackendAdapter` (providers). The hardcoded `memory: Option<Arc<MemoryRuntime>>` field is removed once Memory registers itself as a provider entry; `memory_source()`-style special cases disappear with it.
4. **Explicit activation** — `Zahir::activate(&self, reference) -> Result<BackendLease, ResolveError>` (name/shape to be fixed by Task 1). Semantics: resolve the entry; if the provider is already ready, return a lease; if it is cold, **ask the owner** through its `ProviderActivator`; on success return a fresh lease bound to the new provider generation; on failure return an error and leave the capability unavailable (fail-closed, no stale lease).
5. **Fencing** — a provider lease carries the ownership generation (from `OwnershipTree`) *and* the provider generation (from `ProviderLifecycle`); `validate_lease` re-checks both, so a lease captured before stop/replace fails closed afterwards.
6. **Owner adapters (thin, one per family, outside the Core)**
   - *Memory*: a small adapter maps `MemoryState` onto `ProviderState` in one place (pinned by a test); `MemoryRuntime` keeps its own state machine and keeps owning its bundle.
   - *Terminal*: `PtyManager` already owns a `ProviderHandle` with `Cold → Starting → Ready → Stopping` and an owned driver (`DriverState { generation, handle, stop }`); the adapter supplies the descriptor and the activation hook that calls `PtyManager::activate`.
7. **`ProviderActivator`** — the Core's request boundary: `fn activate(&self) -> Result<(), ActivationError>` implemented by the owner. The Core calls it; the owner performs the effect (R1).

## Proof instance: Terminal / 证明实例：Terminal

Terminal is chosen because it is (a) named in the north star, (b) structurally unlike both plugins and channels, so it cannot dictate the Core's vocabulary, (c) already compliant with the Core's `ProviderHandle` contract, and (d) backed by **real OS resources** — an owned flush driver and PTY sessions — so activation and release are observable rather than bookkeeping.

Evidence this slice must produce (all real-effect, deterministic, no sleeps):

1. **Cold describe** — the capability is visible with `Cold` state and `driver_active == false`, and **no PTY resource exists** (no driver task, no session).
2. **Resolve does not activate** — resolving the capability yields a lease while the provider stays `Cold` (the existing "read/resolve never starts" rule is preserved).
3. **Explicit activation** — `activate` moves the provider to `Ready`, the owned driver starts, and the returned lease validates.
4. **Stop releases resources** — stopping the provider joins the driver and closes sessions; the previously captured lease then fails closed.
5. **Replacement fences the old lease** — a second activation after stop yields a **new provider generation**, and the old lease stays invalid.
6. **Generic-path regression net** — the existing Memory capability tests (`projection_host_memory_tests`, `zahir_facade_memory_tests`) and the Tool-mount evidence stay green while Memory is served by the generic path.

## Evidence discipline / 证据纪律

- Real-effect tests only: real router/Core, real owner, real resources. No sleeps, no scheduler luck, no global mocks, no new public test-only API.
- Synthetic provider owners may live in `#[cfg(test)]` modules inside the Core; they are not public API and not a second lifecycle authority.
- Every Cargo command is preceded by `python qa/browser_live/check_memory.py` with `ALLOW` and ≥ `4294967296` available bytes; `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`; focused runs use `--test-threads=1` and must select a positive number of tests.
- No unrelated full-suite failure may be repaired; any new failure must be attributed by name against the slice's task-start commit.

## Remaining gaps after this slice / 本切片之后的剩余缺口

Gateway channels, Feishu, MCP, browser, sandbox and computer-use are still **not registered** with the Core; EffectClaim producers, external-effect exactly-once, Safe Replay and a universal durable scheduler remain deferred; binary slimness (code genuinely not linked) is untouched; cross-process/Bridge activation semantics are out of scope; the remaining derived-consumer / RPC operation-escape census from the Memory slices is still a separate slice.

## What is left to the builder / 留给实施者的决定

Exact module split and type names; the `MemoryState → ProviderState` mapping table and where it lives; the activation error set and whether `activate` belongs on the `Zahir` trait or on a separate trait; the startup wiring order (who registers the Memory entry, who registers the Terminal entry, and when); whether `ProviderRegistry` needs its own revision cursor for `subscribe`.

## Non-claims / 不宣称

This slice does not claim that any subsystem is unloadable from the binary, that all writers stop, that effects are exactly-once, that providers are safely replaceable in general, or that channels/Feishu/MCP/browser/sandbox are integrated. It claims exactly one thing: the Core can see, resolve and explicitly start a second kind of capability, and a lease fails closed when that provider is replaced.
