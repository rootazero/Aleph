# Capability Core Remaining Roadmap

> This is the master roadmap for the unfinished Capability Core work after the first contract-first vertical slice. Execute one gate at a time. A failed contract gate stops the dependent implementation.

**Goal:** Evolve Aleph into a lightweight Capability Core with providers that can be discovered, started, stopped, replaced, subscribed to, and recovered without copying subsystem authority or initializing disabled resources.

**Current baseline:**

- Repository: resolve the current checkout with `git rev-parse --show-toplevel`.
- Branch: `main`
- Verified local HEAD (2026-10-26): `fe6104244` (`feat(capability): establish provider contract slice`).
- The first facets/source-snapshot vertical slice is committed in this baseline. The initial working tree was clean; remote state was not refreshed during this assessment.
- `CapabilityKind` remains the nine existing kinds.
- `CapabilityFacet` exists for `Callable`, `EventSource`, `Subscription`, `ProviderLifecycle`, and `ExternalEffect`.
- `CapabilityBackend::snapshot(&Scope) -> CapabilitySnapshot` is the read-side source contract for Tool.
- Tool is the only production live backend.
- `ProviderHandle` owns provider state, instance identity, generation, and admission fencing.
- `OwnershipTree` owns visibility binding, owner generation, revoke, and dispose.
- `ToolHandlerRegistry` owns Tool descriptors, handlers, publication revision, and live notifications.
- `SessionEventStore` owns durable session events and recovery evidence.
- Capability tests: 214 passed in `cargo test -p alephcore --lib capability::`.
- Feishu has no Capability descriptor, resolver, production mount, or Zahir route.

**Success criteria from the original roadmap:**

1. Existing registries and duplicated structures are removed or replaced at least as fast as new Core structures are introduced.
2. At least one real provider exposes more than one behavior facet through one descriptor and one source-backed projection.
3. At least one provider/resource leak is structurally eliminated by ownership/lifetime disposal and provider cleanup through the existing lifecycle owner; query `Scope` is not a disposer and shared providers are not implicitly stopped.
4. At least one recovery path is driven by a descriptor-backed replay/effect contract.
5. `src/harness/**` remains unchanged.
6. Disabled providers do not initialize threads, connections, databases, caches, token clients, listeners, or websockets.

## Dependency graph

```text
R0 baseline and authority census
  -> R1 generic provider/source/lease design gate
      -> R2 Feishu A/B source-backed re-gate
          -> R3 Feishu descriptor + lifecycle + callable/event projection
              -> R4 EventSource/Subscription production semantics
              -> R5 external effect + durable recovery
                  -> R6 Terminal/Memory/Sandbox migrations
                      -> R7 workspace and live-boundary verification
```

`R4` may begin with a non-Feishu fixture, but it cannot be declared complete until one real provider uses the production source and subscription boundaries. `R5` is a separate gate because inbound/outbound Feishu behavior must not be labeled `ExternalEffect` before claim, drain, and recovery semantics exist.

## R0 — Preserve the current vertical slice

**Purpose:** Establish a clean review baseline before extending the Core.

**Checks:**

- Re-run `cargo fmt --all --check`.
- Re-run `git diff --check`.
- Re-run `cargo check -p alephcore --tests`.
- Re-run `cargo clippy -p alephcore --tests -- -D warnings`.
- Re-run `cargo test -p alephcore --lib capability::`.
- Confirm no change under `src/harness/**`.
- Confirm no Feishu production route was added.

**Stop condition:** Any failed check or unexplained diff must be resolved before R1.

## R1 — Generic provider/source/lease design gate

**Purpose:** Decide how non-Tool capabilities enter the existing Core without making a registry of registries.

**Required design decisions:**

- Whether `CapabilityBackend` remains the only read-side source trait or needs a zero-state extension for source identity/reference.
- How a descriptor points at a provider instance without copying provider state.
- Whether the lease remains source-specific behind a common validation envelope, or whether a second real callable provider proves a generic lease shape.
- Which authority owns explicit start/stop for a provider. Core may validate and project lifecycle state, but it must not silently become a second lifecycle supervisor.
- How `ProviderHandle` instance identity and generation are carried into use-time validation.
- How owner generation, source revision, provider generation, and `SessionEvent.seq` remain separate.
- What `Scope::dispose` is allowed to stop and what it must leave to the provider authority.

**Required artifacts:**

- Design spec with type/signature candidates and rejected alternatives.
- Authority table listing `ChannelRegistry`, `ProviderHandle`, `OwnershipTree`, `ToolHandlerRegistry`, `SessionEventStore`, and future source owners.
- Replacement and late-completion matrix.
- Read-side side-effect contract.
- A fixture contract that can prove cold/discovered providers allocate no resources.

**Recommended decision:** Keep Tool's `BackendLease::canonical_entry` source-specific. Introduce a common validation envelope only if the second provider demonstrates the need; do not force Feishu into a Tool-shaped lease. Reuse `CapabilityBackend`, `ProviderHandle`, and `OwnershipTree`; do not add `CapabilitySource` registry or Core lifecycle registry.

**Gate:** R1 is GO only when an implementer can describe a non-Tool provider without adding a parallel registry, handler map, owner map, or durable store. Otherwise return `R1 NO-GO / deferred` and revise the design.

### R0/R1 assessment — 2026-10-26 / 本轮裁决

R0 **PASS**: fmt, diff check, `cargo check -p alephcore --tests`, strict Clippy, and `cargo test -p alephcore --lib capability::` all exited 0; **214 passed, 0 failed**. Test linking emitted a macOS `__eh_frame` size warning.

R1 **bounded lifecycle + source-capture slice PASS; overall NO-GO / deferred**. The authorized slice generalized `ChannelRegistry` replacement/unregister teardown from a Feishu-only gate to the existing common channel owner. Two replacement tests were RED before the change; after the private `stop_outgoing_channel` helper, three focused lifecycle tests and the full 44-test `gateway::channel_registry::tests::` filter passed. Failed stop retains the incumbent registration and webhook mount. No new registry, handler map, owner map, or durable store was added.

The next bounded R1 slice added crate-private `ChannelCapture` at `src/gateway/channel_registry.rs`: one lifecycle boundary captures the registered channel `Arc` identity plus the channel-source publication revision, and the use boundary validates the current `Arc` identity through the same registry. Three capture tests were RED before implementation and pass after implementation. The capture is read-only, does not start/authenticate/network the channel, unrelated publication does not invalidate it, and replacement/unregister rejects stale captures.

首个未闭合 provider 契约已收窄为：把这个 source-owned capture 与实际 `ProviderHandle` instance/generation、descriptor identity/schema/revision、`OwnershipTree` owner generation 和 admitted-work completion acceptance 组成同一份 source proof。provider resource receipts 仍未闭合。R2 未进入；没有创建 Feishu descriptor、resolve route、production mount 或 Zahir route。

The first unclosed provider contract is now narrower: bind this source-owned capture to the actual `ProviderHandle` instance/generation, descriptor identity/schema/revision, `OwnershipTree` owner generation, and admitted-work completion acceptance as one source proof. Provider resource receipts remain open. R2 was not entered; no Feishu descriptor, resolve route, production mount, or Zahir route was created.

This ruling does not require implementing a universal lease to pass a design gate. The next R1 revision must ground the source-specific provider binding in existing authorities, then reconsider R1 before any R2 work.

**R1 slice ledger:**

- [x] Common `ChannelRegistry` replacement/unregister teardown is source-owned and tested for every channel type.
- [x] Bounded `ChannelCapture` records channel instance identity and source publication revision, then validates use through `ChannelRegistry`.
- [ ] Bind source capture to `ProviderHandle` instance/generation and descriptor identity/schema/revision.
- [ ] Define owner-generation and admitted-work completion acceptance without a second map or durable store.
- [ ] Produce resource receipts proving disabled/discovered read paths do not initialize provider runtime resources.

## R2 — Feishu A/B source-backed re-gate

**Purpose:** Re-evaluate Feishu against the R1 contract before writing Feishu integration code.

**A must prove:**

- One existing `CapabilityKind` plus facets can describe Feishu's actual behavior without Tool masquerading.
- The descriptor is derived from an existing canonical Feishu/Gateway source, not a new descriptor adapter or copied registry.
- Identity, schema/version/fingerprint, route metadata, and visibility have one authority each.
- Inbound event source, outbound callable operations, provider lifecycle, and external effect are distinguishable.

**B must prove:**

- Provider instance and provider generation come from `ProviderHandle`.
- Channel instance, desired state, factory, start/stop, and webhook mount remain owned by `ChannelRegistry`.
- Owner generation comes from `OwnershipTree`.
- Source revision comes from the canonical source snapshot.
- `BackendLease` or its approved successor validates all required domains without copying them.
- Replacement, revoke, dispose, webhook admission, outbound call admission, accepted handler ownership, drain, and late completion have source-backed semantics.

**Required read-side proof:**

- Feishu discovery/list/project/describe/resolve does not start a channel, authenticate, obtain a token, create a connection, open a listener, start a websocket, or allocate provider runtime resources.
- `resolve` does not imply start.
- A provider not in `Ready` is fail-closed.

**Stop condition:** If A or B has one unclosed contract, return `B8.4 NO-GO / deferred`. Do not create Feishu adapter glue, a Feishu facade, a resolve route, a production mount, or a test bypass.

## R3 — Feishu production capability slice, only after R2 GO

**Purpose:** Make Feishu the first real multi-facet provider.

Implement in separate, reviewable tasks:

1. Descriptor projection from canonical `ChannelRegistry`/Feishu provider state.
2. A source-backed snapshot and revision boundary.
3. Visibility/owner binding using the existing `OwnershipTree`.
4. Provider generation and use-time admission using the existing `ProviderHandle`.
5. Lifecycle projection that observes existing `ChannelRegistry` start/stop and does not duplicate it.
6. Outbound callable projection for send/react/edit/delete, with operation-specific semantics.
7. Inbound `EventSource` projection for webhook/WS admission.
8. Subscription semantics only after the source and observer/recovery contract is explicit.
9. Replacement/unregister/revoke/dispose and late-completion tests.

`ExternalEffect` must remain absent until R5 closes its producer and recovery semantics. A Feishu capability can expose callable/event/lifecycle facets without claiming exactly-once external effects.

**R3 acceptance:** One Feishu descriptor, one source snapshot, one owner binding, one provider generation, and one production mount path are observable without a second authority. Disabled Feishu does not initialize runtime resources.

## R4 — EventSource and Subscription production semantics

**Purpose:** Close the inbound event and observer boundaries without confusing live notification with durable recovery.

**Required decisions:**

- Whether webhook/WS admission is an `EventSource` capability, a provider-owned input surface, or both through one descriptor.
- How subscriptions identify the source and observer without copying source state.
- Snapshot anchoring, cursor semantics, lag handling, invalidation, and resync.
- Ownership and cancellation of accepted webhook handlers and queued work.
- Which events are live notifications and which are durable `SessionEventStore` evidence.
- How provider replacement invalidates subscriptions and old event handlers.

**Acceptance:** A real inbound provider has tests for admission, lag/resync, replacement, cancellation, and no false durable recovery claim.

## R5 — External effects, EffectClaim producer, and durable recovery

**Purpose:** Close the external-effect boundary before any provider is labeled `ExternalEffect`.

Implement only after a separate design approval covering:

- EffectClaim producer identity and fence source.
- Atomic event batch containing claim, budget reservation, owner generation, and fence.
- Invocation and terminal transitions.
- Provider/API call admission relative to claim state.
- Restart, crash, duplicate request, timeout, and late completion behavior.
- Durable memo and idempotency key ownership.
- Safe Replay eligibility and fail-closed refusal.
- Scheduler ownership and whether a universal scheduler is actually required.

**Acceptance:** At least one real external operation has source-backed claim/recovery evidence. No in-memory retry loop or fire-and-forget producer may be used as a substitute.

## R6 — Migrate remaining providers one at a time

Migrate in this order unless a new dependency review changes it:

1. Terminal: process ownership, start/stop, output source, cancellation, replacement, and resource disposal.
2. Memory/graph: database/cache lifecycle, read-side isolation, indexing/subscription behavior, and durable authority.
3. Sandbox: process/container boundary, capability scope, external effects, and cleanup.

For each provider, require:

- Canonical descriptor/source owner.
- Facet selection justified by observable behavior.
- Lazy disabled state with no resource initialization.
- Provider generation and owner generation separation.
- Replacement and late-completion tests.
- No second registry or durable store.
- A migration diff that deletes or retires the replaced authority in the same phase.

## R7 — Final validation

Run the appropriate checks after each phase, then broaden at the end:

- `cargo fmt --all --check`
- `git diff --check`
- `cargo check -p alephcore --tests`
- `cargo clippy -p alephcore --tests -- -D warnings`
- `cargo test -p alephcore --lib capability::`
- Full workspace `cargo check` and `cargo test` from the correct package/workspace locations.
- Provider-specific integration tests.
- Real boundary tests for disabled startup, replacement, resource disposal, and protocol projections.
- Live TUI testing only for TUI changes.

Do not claim full target completion until R2–R7 evidence exists. Do not use the current Tool-only 214-test result as proof that Feishu or other providers are closed.

## Explicitly deferred until their gates pass

- Feishu descriptor/resolve/mount production wiring before R2 GO.
- Universal non-Tool lease before a second real callable provider proves the required shape.
- `ExternalEffect` facet on Feishu before R5.
- EffectClaim producer, Safe Replay, external exactly-once, and universal scheduler before their independent design gate.
- Terminal, Memory, Sandbox, and other kind backends before their individual authority audits.
- Any modification to `src/harness/**`.
