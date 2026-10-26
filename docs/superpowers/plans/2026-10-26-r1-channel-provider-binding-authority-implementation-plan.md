# R1 Channel Provider Binding Authority Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `ChannelRegistry` 扩展为唯一 source-owned Gateway provider binding authority，用真实 source provenance、生命周期、准入/完成与资源回执闭合本地 R1 slice。 / Establish source-owned binding, lifecycle, admission, completion, and resource evidence in the existing registry.

**Architecture:** Extend the existing canonical registration entry, not a parallel registry; the source mints each provider and owns its actual channel lifecycle. Lifecycle declarations use the existing Resource kind and ProviderLifecycle facet, never Callable/send semantics; internal typed transport work proves fencing and drain without publishing an operation. Trade-off / 代价：registry 必须承担分阶段同步与排空，未迁移的 Bound 兼容调用必须拒绝；未来公开 send 必须独立审批 operation identity/schema，不能复用生命周期声明。

**Tech Stack:** Rust 1.99 / pinned 1.99.1, Tokio, existing serde_json, sha2 and thiserror; existing slot API Channel contract, ProviderHandle, OwnershipTree; existing reqwest/rusqlite only for local resource fixtures. No dependency additions.

**Spec:** `docs/superpowers/specs/2026-10-26-r1-channel-provider-binding-authority-design.md` (approved design supplied for this planning request; read together with current `AGENTS.md`).

## Global Constraints

- 本文件是计划，不是实施授权或 GREEN 证据。 / This document authorizes no execution by itself. The current user supplies the design approval for planning; do not edit its older “Awaiting user approval” status, and do not inherit historical RED/GREEN counts.
- Exactly one canonical channel association: replace the existing `channels` map value with a Legacy/Bound registration entry. No second channel/provider/owner/handler/disposer map or `ChannelProviderRuntime`; entry-local work accounting is not another registration authority.
- ProviderHandle owns provider state/generation only; OwnershipTree owns owner generation/tombstones only. ProviderHandle must not own a channel. The slot API must not depend on Core.
- Bound registration accepts a channel and validated declaration, never a caller-created ProviderHandle. Mint the lightweight provider in the source transaction; never use `ProviderLifecycle::ready` to upgrade a legacy channel.
- Lifecycle descriptor/schema is canonical for this slice. Exact identity namespace: `aleph/gateway/channels`; name is the complete ChannelId, including `/`. Existing Resource kind + ProviderLifecycle facet only; no Callable, send operation, or new CapabilityKind.
- Schema fingerprint hashes canonical lifecycle declaration schema; descriptor fingerprint separately covers the full descriptor. Live state, provider/owner generation, transport payload/result, addresses, Debug output, timestamps and random values are not schema authority.
- No Feishu/R2, `src/harness` changes, universal lease/generalized envelope, EffectClaim producer, Safe Replay, external exactly-once, universal scheduler, durable recovery, production mount, Zahir route or new backend. No all-channel migration claim from two fixtures.
- No production channel opt-in in this slice; legacy factories, boot registration and pure Channel/ChannelFactory APIs keep their contracts. All new operations are crate-private Gateway source operations, not public Capability invocation APIs.
- Read/capture/resolve must not create provider runtime resources. “Zero” means zero provider-runtime **attempt and active deltas**, not zero metadata allocations or zero registry broadcast allocation. Failed attempts count.
- Lock order: source `lifecycle_sync` → OwnershipTree mutex → ProviderHandle lifecycle mutex → entry-local accounting. Acquire needed async map/state guards before synchronous authority callbacks; callbacks contain no await, I/O, plugin code, external handler or lock re-entry. Never acquire source while holding tree/provider/entry locks.
- Never hold source/tree/provider/accounting gates across transport start/stop/send, drain, cancel or join. Retain an entry-local operation marker across these phases; revalidate it before committing.
- Revision reservation is checked before destructive retirement/cleanup and rechecked under the final source gate. Exhaustion returns an error before map mutation; no panic-after-publication, wrap, saturation or reuse. Unrelated publications must not stale an unchanged entry.
- Stop success requires real work drain and cleanup/join; an Arc count, ChannelStatus, unregister Option, or ProviderState alone is not a receipt. Retain incumbent identity and diagnostic residual on failed cleanup, not a fictitious Ready guarantee.
- Actual implementation must be delegated task-by-task to worker subagents. A separate read-only reviewer checks spec compliance and code/tests before the next task; the main agent integrates and verifies, not silently takes over implementation.
- Unmounted crate-private proof APIs may follow the existing ChannelCapture pattern with narrowly scoped, explained `#[allow(dead_code)]` on those specific items; do not add blanket lint suppression or a production mount merely to silence warnings. The final clippy command must still pass.
- All writers run serially in dependency order below. Especially, never parallel-write `src/gateway/channel_registry.rs`, its helper/fixture modules, or the same reference document. Reviewers inspect a frozen task diff; fixes return to that task's worker.
- Before tests, read the applicable installed testing skill and repository testing instructions. Run cargo from the repository root for this Rust package; do not apply unrelated Bun monorepo commands. Before each cargo invocation run `python qa/browser_live/check_memory.py`; proceed only on exit 0, otherwise report environment-blocked.
- Preserve pre-existing work. Planning-time dirty paths include the registry probe, earlier roadmap/ledger/provider-source spec, and the supplied design. Do not stage, revert, rewrite or “clean up” unrelated work. Commit suggestions below apply only after execution approval and reviewed task hunks; no `git add .`.
- Documentation updates are the final task, after implementation evidence. Keep reference additions bilingual; code comments English. Do not edit AGENTS.md/CLAUDE.md, which would require synchronization and is unnecessary here.

## Review Focus

1. 同 ID、同数字代际但不同实例，或 OwnershipTree 同 nonce 覆写 owner/lifetime，仍应拒绝旧证明。 / Equal numbers are not authority: Tasks 3/5/6 pin full identity and binding comparisons.
2. stop 等待的 work 正好要进入 completion 时，不应死锁或提前报 drained。 / Stop-versus-completion ordering: Task 5 tests both linearizations and accounting release.
3. replacement 已退休但 cleanup 失败，或 revision 耗尽，应保留可诊断 incumbent，不能混代发布或复活。 / Failed replacement/publication: Task 6 tests failure retention, retry and exhaustion.
4. 调用方 timeout、旧结果晚到，或 owner revoke/dispose，应只结清原 work，不写 successor，也不自动补发。 / Cancellation and stale completion: Tasks 5/6 test real boundaries, not validation calls alone.
5. 重复只读、失败资源尝试、legacy/Bound 旁路应保持冷态或 fail closed。 / Cold reads and compatibility bypasses: Tasks 7/8 include positive controls and both non-Feishu resource shapes.

---

## Execution Map / 委派与证据边界

**Dependency order:** Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6 → Task 7 → Task 8 → Task 9. This is deliberately serial: all source behavior converges on the same canonical entry. Do not start Task 9 until all implementation/review gates pass.

**Per-task handoff:** give a fresh worker the approved spec, Global Constraints, the task's exact Files/Interfaces, and predecessor signatures. Require RED cause, GREEN commands/results, changed hunks and residual risks. Give a fresh reviewer the same contract and actual diff; require rejection of fabricated provenance, effect-free “receipts”, scope expansion and unverified claims. One task may contain several small RED→GREEN cycles; do not commit a deliberately failing intermediate state. A compilation RED is acceptable for a genuinely missing proposed API, not evidence of runtime correctness.

**Current authorities inspected / 当前源码事实（执行前重读，不依赖移动后的行号）：**

| Path | Authority / execution disposition |
| --- | --- |
| `src/gateway/channel_registry.rs` | Existing ChannelHandle map, source capture, source revision, desired intent, lifecycle gate, start/stop/restart, outbound paths and detached inbound forwarder. Modify in Tasks 1/4–8, never in parallel. |
| `src/capability/provider.rs` | Actual provider lifecycle Arc, checked Starting generation, Ready callback and acquire_call. Modify only Task 2. |
| `src/capability/ownership.rs` | Binding/tombstone authority; register can replace owner/lifetime using the same nonce. Modify only Task 3. |
| `src/capability/backend.rs` | Inspect Tool schema hash and snapshot precedent; no channel backend or edit. |
| `src/capability/zahir_facade.rs` | Inspect full descriptor fingerprint/ownership reconciliation precedent; no mount, route, edit or invoke API. |
| `src/tools/registry.rs` | Inspect source-specific Tool proof and before/after-await validation; regression tests only, no lease generalization or edits. |
| `src/tools/descriptor.rs`, `crates/aleph-slot-api/src/descriptor.rs` | Inspect/reuse the existing `canonicalize_json` re-export, rather than create a second JSON canonicalizer. No edits. |
| `crates/aleph-slot-api/src/channel.rs`, `src/gateway/channel.rs` | Pure Channel/factory and Core re-export/telecom extension; inspect compatibility signatures, no ProviderHandle dependency or edits. |
| `src/session/store.rs` | Inspect durable boundary only; SessionEventStore remains the recovery authority and receives no new producer/event/write. No edits. |
| `src/gateway/webhook_receiver.rs` | Existing projection/mount table and dispatch; inspect mount bool, held handler behavior and unmount semantics. Use existing table, no second handler map and no edits. |
| `src/gateway/handlers/channel.rs`, `src/gateway/handlers/discord_panel.rs` | Inspect config/create/update/delete, raw get and send consumers; no RPC or config migration. |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` | Inspect actual register_for_boot paths (not only factories); no production Bound registration or edits. |
| `src/gateway/channel_health_monitor.rs`, `src/gateway/delivery_queue.rs`, `src/gateway/inbound_router/permission.rs`, `src/gateway/inbound_router/executor.rs`, `src/builtin_tools/channel_manage.rs` | Inspect restart, retry, telecom and raw get consumers; Bound bypasses are rejected centrally, legacy unchanged. No edits. |

**Planned production file boundaries:** `src/gateway/channel_registry.rs` keeps canonical association, synchronization and actual execution; new child `src/gateway/channel_registry/binding.rs` holds source-specific declaration/capture/lease/proof/error types and pure encoding, not another runtime; new `src/gateway/channel_registry/test_support.rs` is `#[cfg(test)]` only. Declare child modules from the existing registry file; no `src/gateway/mod.rs` changes or subsystem-wide restructure.

**Baseline warning:** the existing test `capture_exposes_provider_instance_and_generation_for_admission` creates an unrelated Ready ProviderHandle and references nonexistent capture fields. This is a source-observed compile/provenance defect, not a cargo result from planning. Task 1 replaces that test, not its intent, before recording the new RED. Do not make this false provenance GREEN by adding fields to the legacy capture.

### Task 1: Publish one cold, source-minted canonical binding / 冷态单一来源发布

**Files:**
- Create: `src/gateway/channel_registry/binding.rs`.
- Modify: `src/gateway/channel_registry.rs` (canonical map value, registration/capture/read adapters and tests).
- Test: inline `src/gateway/channel_registry.rs::tests` and `src/gateway/channel_registry/binding.rs::tests`.

**Interfaces:**
- Consumes: existing `Channel`, `ChannelId`, `ProviderLifecycle::new`, `ProviderHandle::new`, `ProviderHandle::same_instance`, `CapabilityDescriptor`, `OwnershipTree`, `canonicalize_json` and sha2. Ownership is supplied as the one `Arc<OwnershipTree>` in each immutable Bound entry; it is not recreated at resolve.
- Produces in `binding.rs`: crate-private `ChannelLifecycleDeclarationV1`, validated `ChannelLifecycleContract`, `ChannelInitialState { Discovered, Cold, Disabled }`, `BoundChannelCapture`, `ChannelBindingError`, `ChannelCleanupResidual`, and `ChannelRuntimePhase`. Keep existing `ChannelCapture` as the legacy source capture, not a provider proof.
- Produces source APIs (all crate-private):

```rust
ChannelLifecycleDeclarationV1::schema() -> serde_json::Value;
ChannelLifecycleContract::new(
    owner: OwnerRef, lifetime: LifetimeScope, visibility: VisibilityScope,
    lifecycle_schema: serde_json::Value, metadata: ProjectionMetadata,
) -> Result<Self, ChannelBindingError>;
ChannelRegistry::register_bound_channel(
    &self, channel: Box<dyn Channel>, contract: ChannelLifecycleContract,
    ownership: Arc<OwnershipTree>, initial: ChannelInitialState,
) -> Result<ChannelId, ChannelBindingError>; // async
ChannelRegistry::capture_bound_channel(
    &self, id: &ChannelId,
) -> Result<BoundChannelCapture, ChannelBindingError>; // async
ChannelRegistry::snapshot_bound_channels(
    &self, visibility: &VisibilityScope,
) -> (Vec<CapabilityDescriptor>, u64); // async, source-local projection only
canonical_schema_fingerprint(schema: &serde_json::Value) -> [u8; 32];
canonical_descriptor_fingerprint(descriptor: &CapabilityDescriptor) -> [u8; 32];
```

- `ChannelBindingError` has source-specific variants `NotFound`, `UnboundLegacy`, `InvalidContract`, `WrongSource`, `WrongProvider`, `WrongVisibility`, `StaleEntry`, `StaleDescriptor`, `Retiring`, `Busy`, `PublicationExhausted`, `Provider(ProviderCallError)`, `ProviderLifecycle(ProviderLifecycleError)` and `Runtime { phase: ChannelRuntimePhase, error: String, residual: ChannelCleanupResidual }`; Task 3 adds `Owner(BindingValidationError)`, Task 5 adds `ExecutionLost`. No new public ResolveError cases or generic envelope.
- `ChannelCleanupResidual` carries `active_work: usize`, `unjoined_forwarder: bool`, `transport_may_be_live: bool`, `mount_retained: bool`; phase/error belong to the Runtime error. It is a diagnostic, never authorization.
- Exact capture field vocabulary for later tasks/tests: `source: Arc<()>`, `entry: Arc<ChannelRegistrationEntry>`, `channel: ChannelHandle`, `source_revision: u64`, `entry_revision: u64`, `descriptor: CapabilityDescriptor`, `descriptor_fingerprint: [u8; 32]`, `provider: ProviderHandle`, `provider_generation: u64`. Define ChannelRegistrationEntry in the parent registry module; capture fields are visible only to the owning source module and its descendants (`pub(super)` from the child), never consumer-settable. The ownership Arc remains in the immutable entry; leases reference that entry, not an independent owner association.

- [ ] **Step 1 — Write RED tests, replacing the false probe.** In registry tests add `bound_registration_mints_canonical_provider_while_cold`: register through the new API; capture twice; assert `same_instance`, generation `0`, state Cold, descriptor revision equals entry revision and original channel Arc is the entry's actual Arc. Test code may observe private entry data, never inject the provider. Add `legacy_capture_never_synthesizes_provider`: register and factory-create legacy channels; ordinary capture remains valid, Bound capture is `Err(UnboundLegacy)`, repeated reads do not create any Bound entry. Add `bound_capture_preserves_full_id_and_source_identity`: IDs such as `team/a/b` remain the complete descriptor name and two registries get distinct private `Arc<()>` source tokens.
- [ ] **Step 2 — Write schema/fingerprint RED tests.** In binding tests add `lifecycle_contract_rejects_placeholder_and_operation_schema`, `lifecycle_schema_fingerprint_is_canonical`, `descriptor_only_change_changes_full_fingerprint`. Assert empty/missing required fields, `{}`, `true` and send input/output schemas are rejected; object-key reorder leaves schema hash equal; array order or schema content changes hash; schema hash is nonzero. Assert changing owner/lifetime/visibility/metadata/revision changes full descriptor hash but not the lifecycle schema hash. A mutated hash input does not imply that an unsupported schema is registrable.
Core assertions after the real registrations/schema setup (the remaining named tests exercise the independent mutations described above):

```rust
assert!(first.provider.same_instance(&second.provider));
assert_eq!(first.provider.snapshot(), (ProviderState::Cold, 0));
assert_eq!(first.descriptor.id.namespace, "aleph/gateway/channels");
assert_eq!(first.descriptor.id.name, "team/a/b");
assert_eq!(first.descriptor.revision.0, first.entry_revision);
assert_eq!(first.descriptor.kind, CapabilityKind::Resource);
assert!(first.descriptor.supports(CapabilityFacet::ProviderLifecycle));
assert!(!first.descriptor.supports(CapabilityFacet::Callable));
assert!(matches!(legacy_bound_capture, Err(ChannelBindingError::UnboundLegacy)));
assert_eq!(canonical_schema_fingerprint(&ordered_a), canonical_schema_fingerprint(&ordered_b));
assert_ne!(canonical_descriptor_fingerprint(&before), canonical_descriptor_fingerprint(&metadata_changed));
assert_eq!(before.schema.fingerprint, metadata_changed.schema.fingerprint);
```

- [ ] **Step 3 — Observe the genuine missing-API RED.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_registration_mints_canonical_provider_while_cold -- --exact` and `cargo test -p alephcore --lib gateway::channel_registry::binding::tests::`; expect compile failure for the not-yet-defined Bound API/types, not the removed unrelated-provider assertions.
- [ ] **Step 4 — Implement declaration and cold canonical publication.** Define v1 schema as an object with `additionalProperties: false`, required `declaration_kind`, `channel_id`, `provider_transport`, `lifecycle_model_version`; their schemas are respectively `const: "channel-provider-lifecycle"`, `type: "string", minLength: 1`, `const: "in_process"`, and `const: 1`. Its `$schema` is `https://json-schema.org/draft/2020-12/schema`, `$id` is `aleph:gateway:channel-lifecycle-declaration:v1`. These describe declaration data, not operation arguments. `new` accepts only canonical-equivalent v1 schema and fixes SchemaRef.version to `1`; no arbitrary caller operation schema. Owner/lifetime/visibility/metadata are descriptor declarations, not invented send permissions.
- [ ] **Step 5 — Implement the entry change and encoding.** Change the one map to `HashMap<ChannelId, Arc<ChannelRegistrationEntry>>`; each entry owns its ChannelHandle and either Legacy or Bound data. Bound immutable publication data contains channel Arc, source token, entry revision, contract/schema/descriptor/full hash, source-minted ProviderHandle and the supplied ownership Arc; mutable lifecycle/work data is entry-local. Mint provider ID from `aleph/gateway/channels/<full-id>`, InProcess transport, using new/Cold/Disabled transitions, never ready(). Capture includes current source cut plus entry identity/revision and observed provider generation; private fields and constructors prevent consumer assembly. Precheck checked source revision under the gate; publish the entire tuple and desired Stopped intent in one cut. Keep legacy signature/return behavior. Existing legacy capture validation uses source token/entry identity, not global-revision equality. Initially reject Bound activation/transport with explicit unsupported/NotReady errors until Tasks 4/5, never fall through legacy raw-handle code.
- [ ] **Step 6 — Make reads and fingerprints exact.** Snapshot filters the exact declared VisibilityScope for this internal source view and includes only complete Bound declarations, no owner creation; unknown/mismatched scope returns no entry. Use existing `canonicalize_json` for canonical bytes and SHA-256. Full descriptor encoding covers every field: id, kind, schema version/hash, revision, owner, lifetime, visibility (all six fields), services (target/relation, array order), metadata.surface and facets. Use explicit stable tagged enum strings and null/array encoding; no Debug/derived Hash or address encoding. V1 produces no services, metadata.surface defaults None, and only ProviderLifecycle facet. Task 6 adds live replacement/update; until then duplicate Bound publication fails closed with Busy, preserving incumbent.
- [ ] **Step 7 — GREEN, review, commit.** Run `cargo test -p alephcore --lib gateway::channel_registry::binding::tests::`, `cargo test -p alephcore --lib gateway::channel_registry::tests::`, and `cargo check -p alephcore --tests`. Require all pass, inspect every map-value adapter for legacy regressions and Bound handle leakage. Commit suggestion: `feat(gateway): mint canonical channel lifecycle bindings` (only the two task files/reviewed pre-existing registry hunks).

### Task 2: Hold provider validation through local commits / Provider 原子验证

**Files:**
- Create: none.
- Modify: `src/capability/provider.rs`.
- Test: inline `src/capability/provider.rs::tests`.

**Interfaces:**
- Consumes: existing lifecycle mutex, `ProviderCallError` struct, checked generation and transition state machine. Do not change Tool `acquire_call` or `with_ready_commit` semantics.
- Produces crate-private provider methods:

```rust
ProviderHandle::with_call_admission<R>(
    &self, expected_generation: u64, commit: impl FnOnce() -> R,
) -> Result<R, ProviderCallError>;
ProviderHandle::with_admitted_completion<R>(
    &self, expected_generation: u64, commit: impl FnOnce() -> R,
) -> Result<R, ProviderCallError>;
```

- Completion guard checks the provider domain only: same generation, Ready **or** Stopping. A genuine non-clone work ticket is required and checked by the source caller in Task 5; this primitive is not standalone permission to complete in Stopping.

- [ ] **Step 1 — Write RED tests.** Add `admission_commit_runs_only_for_ready_current_generation`, `completion_guard_accepts_same_generation_ready_or_stopping`, `provider_guard_serializes_stop_with_commit`, `generation_exhaustion_does_not_run_commit`. Assert callbacks run exactly once on valid state; zero on Discovered/Cold/Disabled/Starting/Stopped/Failed, stale generation or exhausted Starting. A controlled thread/barrier race asserts the stop transition cannot linearize inside a locked admission callback; callback only adjusts local test state, no re-entry.
Core guard assertions (set the test provider Ready at its real generation, then transition it through Stopping):

```rust
assert!(provider.with_call_admission(generation, || 7).is_ok());
provider.transition(ProviderState::Stopping).unwrap();
assert!(provider.with_call_admission(generation, || panic!("must not admit")).is_err());
assert_eq!(provider.with_admitted_completion(generation, || 7).unwrap(), 7);
assert!(provider.with_admitted_completion(generation + 1, || panic!("stale")).is_err());
```

- [ ] **Step 2 — Run RED.** Run `cargo test -p alephcore --lib capability::provider::tests::`; expect missing guard methods, then intended state/ordering assertion failures as implementation proceeds.
- [ ] **Step 3 — Implement the two signatures.** Hold the existing lifecycle mutex through validation and callback. Admission uses the existing Ready+generation predicate; completion uses the narrower caller-enforced ticket contract above and Ready/Stopping+generation. Return existing ProviderCallError fields on rejection. Do not add drain/accounting or channel storage to ProviderHandle. Retain notifications and existing checked generation behavior.
- [ ] **Step 4 — GREEN and compatibility verification.** Run `cargo test -p alephcore --lib capability::provider::tests::` and `cargo test -p alephcore --lib tools::registry::tests::`. Require all pass; reviewer checks lock order with existing Tool commits and confirms no callback is plugin-supplied.
- [ ] **Step 5 — Commit.** Suggest `feat(capability): guard source work admission and completion`; stage only `src/capability/provider.rs`.

### Task 3: Validate the full live ownership binding / 完整 owner binding

**Files:**
- Create: none.
- Modify: `src/capability/ownership.rs`, `src/gateway/channel_registry/binding.rs` (typed error conversion only).
- Test: inline `src/capability/ownership.rs::tests`.

**Interfaces:**
- Consumes: actual OwnershipTree binding keyed by capability + exact VisibilityScope, owner/lifetime, independent OwnerGeneration and tombstones; Task 1's ChannelBindingError.
- Produces crate-private `BindingValidationError { MissingBinding, Revoked, Disposed, GenerationMismatch, BindingMismatch }` and:

```rust
OwnershipTree::with_current_binding<R>(
    &self, capability: &CapabilityId, visibility: &VisibilityScope,
    expected: OwnerGeneration, owner: &OwnerRef, lifetime: &LifetimeScope,
    commit: impl FnOnce() -> R,
) -> Result<R, BindingValidationError>;
```

- Return a dedicated binding-validation error rather than widening existing public `OwnershipError` (currently generation exhaustion, exhaustively consumed by diagnostic control). This preserves that API while implementing the design's proposed full-binding guard. Add `ChannelBindingError::Owner(BindingValidationError)` only; do not change the public facade ResolveError.

- [ ] **Step 1 — Write RED tests.** Add `current_binding_commit_checks_generation_owner_and_lifetime`, `same_nonce_register_replacement_rejects_old_binding`, `binding_guard_fails_closed_for_visibility_missing_revoke_dispose`, `binding_guard_serializes_revoke_after_commit`. Assert `tree.generation` is unchanged across same-nonce register owner/lifetime replacement yet old expected binding returns BindingMismatch and callback count stays zero; changing each field independently fails. Identical re-registration with the same generation/owner/lifetime remains the same authorization fact, as the spec permits. Missing or wrong-visibility binding returns MissingBinding; revoke/dispose tombstones reject and repeated queries never restore registration. Barrier-controlled commit-before-revoke is accepted once; revoke-before-commit never runs callback.
Core same-nonce assertion after replacing the live registration's owner/lifetime, while preserving the old expected values:

```rust
assert_eq!(tree.generation(&id, &visibility), Some(old_generation));
assert!(matches!(
    tree.with_current_binding(&id, &visibility, old_generation, &old_owner, &old_lifetime,
        || panic!("replaced binding must not commit")),
    Err(BindingValidationError::BindingMismatch)
));
```

- [ ] **Step 2 — Run RED.** Run `cargo test -p alephcore --lib capability::ownership::tests::`; expect missing guard/error, then full-binding assertions to fail until implemented.
- [ ] **Step 3 — Implement under one existing tree lock.** Check revocation, disposed lifetime coverage using the existing rank/guard semantics, exact binding existence, generation equality and full owner/lifetime equality; only then run the internal synchronous callback without releasing the tree mutex. Do not call public generation/lookup while already locked, do not bump implicitly, and do not create a second owner map or restore disposed/revoked bindings.
- [ ] **Step 4 — GREEN and independent review.** Run `cargo test -p alephcore --lib capability::ownership::tests::` and `cargo test -p alephcore --lib capability::`. Require all pass; explicitly review the same-nonce case, all visibility dimensions, public error compatibility and absence of source callbacks entering tree in reverse order.
- [ ] **Step 5 — Commit.** Suggest `feat(capability): validate live ownership binding atomically`; stage only the two task files.

### Task 4: Couple Bound start/stop to the actual provider and resources / 真实生命周期耦合

**Files:**
- Create: `src/gateway/channel_registry/test_support.rs` (initial cold transport fixture and deterministic boundary tracker).
- Modify: `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs` (entry-local lifecycle operation/residual data).
- Test: inline `src/gateway/channel_registry.rs::tests`; fixture support is not a production provider.

**Interfaces:**
- Consumes: Task 1 canonical entries/captures/initial state, Task 2 Ready commit/guards, Task 3 immutable ownership reference; existing WebhookMountTable::mount returns bool and must be checked.
- Produces crate-private source methods:

```rust
ChannelRegistry::start_bound_channel(&self, id: &ChannelId)
    -> Result<(), ChannelBindingError>; // async
ChannelRegistry::stop_bound_channel(&self, id: &ChannelId)
    -> Result<(), ChannelBindingError>; // async
ChannelRegistry::restart_bound_channel(&self, id: &ChannelId)
    -> Result<(), ChannelBindingError>; // async
ChannelRegistry::bound_cleanup_residual(&self, id: &ChannelId)
    -> Result<ChannelCleanupResidual, ChannelBindingError>; // async, pure read
```

- Entry-local operation marker is a unique Arc identity, not a second generation allocator. Store owned forwarder cancellation/JoinHandle in the original entry, not a global map. Task 5 extends stop's zero-work drain to admitted work.
- Test support produces `ResourceTracker` (instance-local Arc, injected at real resource boundaries), `ResourceSnapshot`, and a cold HTTP/webhook-shaped fixture; its real start/stop receipt lets the test observe the source-minted provider state, not supply a replacement provider. Snapshot aggregate fields used below are `provider_runtime_attempts: usize`, `active_resources: usize`, `forwarders_spawned: usize`, `forwarders_joined: usize`, `transport_calls: usize`, `tickets_minted: usize`; retain per-resource counters underneath, because aggregate zero/positive assertions alone do not prove each resource kind.

- [ ] **Step 1 — Write RED tests.** Add `bound_start_stop_use_the_registered_provider_instance`, `bound_ready_waits_for_actual_start_and_mount`, `bound_mount_failure_cleans_started_resources`, `bound_restart_changes_only_provider_generation`, `bound_forwarder_is_cancelled_and_joined`. Assert the captured handle is the one seen by lifecycle, generation increments exactly on Starting, Ready is impossible while the real start barrier is held, invalid/colliding mount returns Runtime and no Ready, stopped active resources are zero and joined counts match spawns. Restart keeps entry/schema/owner identity, increments provider generation, and never doubles the forwarder. Add `cancelled_start_retains_retryable_cleanup_residual`: cancellation after resource creation must not silently clear the operation or report Stopped/clean.
Core lifecycle assertions after the controlled real start/stop boundaries:

```rust
assert_eq!(capture.provider.state(), ProviderState::Starting);
// Release the fixture's actual start barrier, then await source activation.
assert!(start_result.is_ok());
assert_eq!(capture.provider.snapshot(), (ProviderState::Ready, 1));
assert!(registry.stop_bound_channel(&id).await.is_ok());
assert_eq!(capture.provider.state(), ProviderState::Stopped);
assert_eq!(stopped.active_resources, 0);
assert_eq!(stopped.forwarders_spawned, stopped.forwarders_joined);
```

- [ ] **Step 2 — Run RED.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_start_stop_use_the_registered_provider_instance -- --exact` and `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_forwarder_is_cancelled_and_joined -- --exact`; expect missing lifecycle methods, not test-injected Ready state.
- [ ] **Step 3 — Implement phased activation.** Under source gate select entry, reject retiring/busy, install marker and transition its real provider to Starting. Drop gates, call that channel's start and prepare managed forwarder/mount; preserve the bool mount failure. Reacquire async entry/map guards before the short provider `with_ready_commit`, revalidate entry/marker/generation and commit local activated state/Running intent only after real setup. Release all sync guards before any await. If setup fails, perform actual cleanup; publish Failed plus true residual if cleanup is incomplete, never Ready based solely on channel.start success.
- [ ] **Step 4 — Implement phased stop/restart without external work yet.** Mark stopping and desired Stopped; cold/disabled no-runtime stop is a resource-free idempotent no-op with no illegal provider transition. Ready transitions Stopping, then gates are released before drain/real stop/cancel/join. On full success unmount the incumbent before publishing Stopped; on stop failure retain its mount, entry and diagnostic residual, with no automatic Ready. Restart requires successful stop before next Starting. A cancelled lifecycle future retains marker/residual; the next explicit stop resumes cleanup for that entry rather than waiting on a lock abandoned by the cancelled future. Cleanup retries may use the existing Failed→Stopped transition after verified cleanup; never invent a new allowed provider transition.
- [ ] **Step 5 — Keep temporary and permanent bypasses closed.** Dispatch Bound start/stop/restart compatibility methods through these source methods, translating Result to ChannelResult at the legacy surface. Until Task 5 there is no admitted transport API; all Bound effect methods still reject. For Bound inbound resources, retain a managed forwarder but do not dispatch its messages into legacy unified inbound consumers; mount a deny-only source-owned handler wrapper, not the raw channel handler. This is an intentional no-ingress guarantee for unapproved operations, not production channel enablement. Legacy forwarder/mount behavior remains unchanged. Task 7 proves this policy at actual dispatch.
- [ ] **Step 6 — GREEN, review, commit.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::` and `cargo check -p alephcore --tests`. Require resource/lifecycle assertions pass and no await under authority guards; independent reviewer checks failure/cancel retry and owned forwarder join. Suggest `feat(gateway): couple bound channel lifecycle to provider` for the three task files.

### Task 5: Admit typed transport work and accept completion atomically / 准入、完成与排空

**Files:**
- Create: none.
- Modify: `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, `src/gateway/channel_registry/test_support.rs`.
- Test: inline `src/gateway/channel_registry.rs::tests`.

**Interfaces:**
- Consumes: Tasks 1–4 entries, ownership guard, provider guard, actual lifecycle and fixtures. No Tool CanonicalEntryLease or facade BackendLease.
- Produces source-specific `ChannelAdmissionLease` (cloneable observation, private fields), `AdmittedChannelWork` (non-Clone, non-Copy, private ticket and original entry), and `ChannelWorkCompletion { Accepted(ChannelResult<SendResult>), DiscardedStale }`; this result is a local handoff, not a durable receipt.
- Produces crate-private methods:

```rust
ChannelRegistry::resolve_channel_admission(
    &self, capture: &BoundChannelCapture, visibility: &VisibilityScope,
) -> Result<ChannelAdmissionLease, ChannelBindingError>; // async
ChannelRegistry::admit_channel_work(
    &self, lease: ChannelAdmissionLease,
) -> Result<AdmittedChannelWork, ChannelBindingError>; // async, source-internal
ChannelRegistry::complete_channel_work(
    &self, work: AdmittedChannelWork, result: ChannelResult<SendResult>,
) -> ChannelWorkCompletion; // async, source-internal, consumes ticket exactly once
ChannelRegistry::send_bound(
    self: &Arc<Self>, lease: ChannelAdmissionLease, message: OutboundMessage,
) -> Result<ChannelWorkCompletion, ChannelBindingError>; // async, internal evidence path
```

- Exact lease fields: `capture: BoundChannelCapture`, `visibility: VisibilityScope`, `owner_generation: OwnerGeneration`; tree identity is the capture entry's ownership Arc. AdmittedChannelWork owns that lease plus one opaque, entry-local ticket; only the source constructs or consumes it. Lease records the exact capture tuple, visibility, tree identity and independently observed OwnerGeneration. Resolve checks but grants no ticket; admit revalidates everything. Add ChannelBindingError::ExecutionLost for a failed owned execution task, retaining its original-entry residual as necessary.

- [ ] **Step 1 — Write provenance/domain RED tests.** Add `bound_work_reaches_transport_only_after_source_admission`, `admission_rejects_wrong_source_and_equal_number_foreign_provider`, `unrelated_publication_keeps_unchanged_entry_lease_valid`, `owner_bump_and_same_nonce_replacement_reject_old_work`. Assert denied admission produces zero tickets/transport calls. In-module adversarial tests may tamper with private capture clones to test rejection, but valid proofs always originate at register/capture/resolve. Independently change source token, entry Arc, provider Arc (same ID/generation), provider generation, descriptor hash/revision, visibility and owner generation/binding; each rejection leaves transport untouched. No independent ProviderHandle enters the production registration API.
- [ ] **Step 2 — Write ordering/drain RED tests.** Add `stop_before_admission_never_calls_send`, `admission_before_stop_drains_and_accepts_pre_stop_completion`, `completion_before_revoke_remains_accepted`, `revoke_before_completion_discards_and_releases_original_work`, `dispose_before_completion_does_not_stop_shared_provider`. Use oneshot/barrier control at real send entry, transport return and acceptance; assert stop future is pending while send is held, then Accepted in same-generation Stopping and stop completes with active_work 0. Reverse the stop/admit order and assert no send. Revoke/dispose first gives DiscardedStale, no local accepted result; completion first stays accepted, with no retroactive removal. Revoke/dispose does not itself stop the shared provider or change source revision.
- [ ] **Step 3 — Write cancellation/error RED tests.** Add `caller_cancel_does_not_release_running_transport_ticket`, `transport_error_is_accepted_as_error_and_releases_ticket`, `unconfirmed_execution_loss_preserves_residual`. Drop/timeout the caller while the owned execution task is held at real transport; stop remains pending and active_work remains 1 until actual return/confirmed cancellation. An Err transport result is Accepted(Err), not success or implicit retry. If task loss cannot prove all transport descendants ended, retain residual and do not report drained; no receipt is inferred from dropping an arbitrary Future.
Core assertions for the stop-first and admit-first cases, each using a fresh fixture and real barriers:

```rust
assert!(matches!(denied, Err(ChannelBindingError::Provider(_))));
assert_eq!(after_denied.transport_calls, 0);
assert_eq!(after_denied.tickets_minted, 0);
assert_eq!(registry.bound_cleanup_residual(&id).await.unwrap().active_work, 1);
// After releasing the admitted transport during ordinary Stopping:
assert!(matches!(completion, ChannelWorkCompletion::Accepted(Ok(_))));
assert_eq!(registry.bound_cleanup_residual(&id).await.unwrap().active_work, 0);
// A separately orchestrated revoke-before-complete case:
assert!(matches!(revoked_completion, ChannelWorkCompletion::DiscardedStale));
```

- [ ] **Step 4 — Run RED.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_work_reaches_transport_only_after_source_admission -- --exact`, `cargo test -p alephcore --lib gateway::channel_registry::tests::admission_before_stop_drains_and_accepts_pre_stop_completion -- --exact`, and `cargo test -p alephcore --lib gateway::channel_registry::tests::caller_cancel_does_not_release_running_transport_ticket -- --exact`; expect missing APIs then specific boundary/accounting failures.
- [ ] **Step 5 — Implement observation and atomic admission.** Resolve reads the existing tree generation (missing means fail closed), then validates under the same source→tree→provider ordering used for admission, without incrementing work. Admit checks source token, live entry/channel identity, entry revision and full descriptor fingerprint, same provider instance and generation, exact visibility and full tree binding; reject retiring/busy/non-Ready. Only inside both authority guards mint the unique entry-local ticket and increment work. Ticket creation is the admission linearization point; never check then unlock before bookkeeping. Run actual I/O outside all authority gates using the proof's channel, never re-get by ChannelId.
- [ ] **Step 6 — Own execution and finalize the original entry.** `send_bound` admits then immediately spawns an owned Tokio execution task containing the proof, registry Arc and transport future; caller cancellation only drops its wait, not transport ownership. Its output is moved into a local completion slot under the source→tree→provider→entry boundary; return Accepted only from that slot. Completion permits same-generation Stopping only for a live, pre-stop ticket on the non-retiring entry. All stale/error paths settle exactly that original ticket, not the current entry found by ID. A guard dropped before dispatch releases a never-started ticket; a running guard lost without confirmed quiescence records residual rather than fabricating zero. Confirmed ended transport is settled exactly once on every verdict; notification wakeups use a register-before-check loop to avoid lost drain wakeups.
- [ ] **Step 7 — Extend stop's actual drain.** Mark Stopping under source gate and release it before waiting for original-entry active work. Ordinary graceful stop leaves entry non-retiring, allowing its real pre-stop tickets to finish. Reacquire gate/marker only for short commits; cleanup and joins run outside it. New transport attempts require new admission; the internal send path performs one typed attempt, no automatic retry or delivery-store writes. Existing authorization/telecom requirements are not bypassed: this internal path is restricted to trusted, authorized fixture/composition use and is not publicly exposed.
- [ ] **Step 8 — GREEN, review, commit.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::` and `cargo test -p alephcore --lib capability::`. Require no deadlock, zero tickets before denied transport, exact local handoff ordering and residuals on unconfirmed task loss. Timeouts are deadlock watchdogs only, never scheduling evidence. Suggest `feat(gateway): fence admitted channel work and completion` for the three task files.

### Task 6: Retire, replace and unregister without mixed publications / 退休、替换与移除

**Files:**
- Create: none.
- Modify: `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, `src/gateway/channel_registry/test_support.rs`.
- Test: inline `src/gateway/channel_registry.rs::tests`.

**Interfaces:**
- Consumes: Task 1 registration/capture/descriptor tuple, Task 4 phased lifecycle and Task 5 ticket/drain/completion.
- Produces completed same-ID replacement semantics in `register_bound_channel` and:

```rust
ChannelRegistry::update_bound_contract(
    &self, id: &ChannelId, contract: ChannelLifecycleContract,
) -> Result<(), ChannelBindingError>; // async, fresh immutable publication
ChannelRegistry::unregister_bound_channel(
    &self, id: &ChannelId,
) -> Result<(), ChannelBindingError>; // async, explicit removal success
```

- Contract-only update retires/drains the old publication before publishing a fresh entry revision/full hash; it may preserve the stopped channel/provider instance but never its old publication identity or old work. Replacement mints a new provider instance. Final map/intent/revision commits are indivisible source cuts.

- [ ] **Step 1 — Write publication RED tests.** Add `replacement_capture_is_complete_old_or_complete_new_tuple`, `contract_update_invalidates_old_publication_not_unrelated_entries`, `source_revision_exhaustion_preserves_incumbent_without_cleanup`. Pause at private preparation, retirement and final commit with test-only barriers; capture sees coherent old (possibly retiring) tuple or coherent successor, never new channel/old provider/schema. Set revision near u64::MAX using in-module test access; final available revision can commit once, next mutation returns PublicationExhausted with identical incumbent entry/intent/mount and no transport stop attempt. Do not expose test mutation APIs in production.
- [ ] **Step 2 — Write retirement/late-result RED tests.** Add `replacement_retirement_discards_old_completion_before_successor_publish`, `unregister_waits_for_work_and_join_before_removal`, `failed_cleanup_retains_incumbent_mount_and_residual`, `late_completion_never_mutates_successor`, `owner_revoke_does_not_publish_or_stop_another_binding`. Assert retiring rejects new admission and old completion is DiscardedStale even while incumbent remains in the map; its original work drains. Unregister reports Ok only after stop/join/unmount and source revision advancement, including when captures still hold Arcs. Stop failure keeps incumbent identity, mount, revision and residual, publishes no successor and never marks Ready. The protocol must prevent successor publication while old real transport is still held: assert replacement is pending, then release old transport and observe DiscardedStale before publication. Delay the caller's observation of that result until after successor publication; consuming the late old result cannot decrement successor accounting, write its handoff, stop it or unmount its handler. Also reuse the old observation lease after successor publication and assert admission rejects before transport. Do not forge/copy an AdmittedChannelWork to simulate an impossible duplicate completion; full drain plus the non-clone proof is precisely what excludes old unaccounted transport after successful publication.
Core failure/publication assertions (snapshot field names are the Task 1 capture contract):

```rust
assert!(matches!(replace_result, Err(ChannelBindingError::Runtime { .. })));
assert!(Arc::ptr_eq(&before.entry, &after_failed.entry));
assert!(before.provider.same_instance(&after_failed.provider));
assert_eq!(before.entry_revision, after_failed.entry_revision);
assert_eq!(before.source_revision, after_failed.source_revision);
assert!(registry.bound_cleanup_residual(&id).await.unwrap().mount_retained);
assert!(!Arc::ptr_eq(&before.entry, &after_success.entry));
assert!(!before.provider.same_instance(&after_success.provider));
assert!(after_success.entry_revision > before.entry_revision);
```

- [ ] **Step 3 — Run RED.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::replacement_capture_is_complete_old_or_complete_new_tuple -- --exact` and `cargo test -p alephcore --lib gateway::channel_registry::tests::failed_cleanup_retains_incumbent_mount_and_residual -- --exact`; expect missing/Busy replacement semantics then intended assertions. Run the exhaustion test separately with its exact full filter.
- [ ] **Step 4 — Implement retirement and successful publication.** Prepare the new declaration/provider privately, then under source gate precheck revision capacity and set incumbent retiring plus operation marker. Release gate, drain its actual work and perform actual stop/join. Do not remove its mount on cleanup failure. On success reacquire gate, verify entry/marker, reserve/recheck checked revision and publish complete successor/intent/revision together; unmount outgoing before successor activation can mount. Never hold the source gate while waiting for a completion that needs it. Concurrent mutations of another ID may consume revision capacity: final exhaustion retains the stopped incumbent with honest state/residual, no successor or wrap; it must never delete the incumbent merely because cleanup already succeeded.
- [ ] **Step 5 — Implement update/removal and compatibility collision behavior.** Contract update and unregister use the same retirement/drain protocol. A failure is retryable only by an explicit lifecycle request operating on that same residual/marker; no automatic resurrection. Preserve existing legacy register/create/unregister signatures: legacy replacement **of a Bound incumbent** is rejected centrally (ID/Option compatibility result must not be interpreted as success by the new API); Bound explicit Result is authoritative. Bound replacement of a legacy incumbent must drain its managed resources or reject if quiescence cannot be proven; do not mint a provider around it to avoid this check. Apply checked prepublication revision allocation to legacy map mutations too, preserving incumbent/no-op semantics on failure rather than panic-after-mutation.
- [ ] **Step 6 — GREEN, review, commit.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::` and `cargo check -p alephcore --tests`. Reviewer checks both linearization orders, contract-only change, cancellation/retry, held Arcs, final exhaustion after unrelated mutation and no successor side effects. Suggest `feat(gateway): retire bound channel publications safely` for the three task files.

### Task 7: Close Bound compatibility bypasses without migrating legacy / 兼容面 fail closed

**Files:**
- Create: none.
- Modify: `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/test_support.rs`.
- Test: inline `src/gateway/channel_registry.rs::tests` and existing unmodified webhook/delivery/health suites.
- Inspect only: `src/gateway/webhook_receiver.rs`, `src/gateway/handlers/channel.rs`, `src/gateway/handlers/discord_panel.rs`, `src/bin/aleph-server/commands/start/builder/subsystems.rs`, `src/gateway/channel_health_monitor.rs`, `src/gateway/delivery_queue.rs`, `src/gateway/inbound_router/permission.rs`, `src/gateway/inbound_router/executor.rs`, `src/builtin_tools/channel_manage.rs`, `crates/aleph-slot-api/src/channel.rs`.

**Interfaces:**
- Consumes: existing public registry signatures and Task 4 deny-only Bound ingress policy, Task 5 explicit proof path, Task 6 retirement.
- Produces no new public APIs. `get(&ChannelId) -> Option<ChannelHandle>` continues to expose Legacy handles only; Bound returns None. Existing ChannelResult-returning Bound transport helpers return `ChannelError::UnsupportedFeature` with an explicit bound-admission-required reason before effects. Existing Option/bool helpers return None/false as appropriate, never a handle/lease. Read-only lists include Bound metadata from its canonical entry.
- Lifecycle compatibility start/stop/restart/all methods dispatch to source-owned Bound lifecycle, not raw Channel. Legacy register/register_for_boot/create remain Legacy and never obtain capability admission by default.

- [ ] **Step 1 — Inventory each existing entry point before changing it.** Classify raw get consumers, boot/config/factory paths, health restart, send/send_attempt/send_public_telecom/private_sms_lease, edit/react/typing/poll/stream/directory helpers, queue retries, webhook handlers and inbound forwarders as read, lifecycle or transport. Keep the inventory in the task review result, not a new registry/catalog or extra document. If a newly discovered raw Bound path cannot be fenced in the listed files, stop for a scoped plan amendment rather than silently modifying a caller subsystem.
- [ ] **Step 2 — Write RED tests.** Add `legacy_apis_remain_legacy_without_provider_binding`, `bound_raw_get_and_transport_helpers_fail_closed`, `bound_webhook_and_forwarder_do_not_dispatch_legacy_ingress`, `bound_compat_restart_uses_same_provider_lifecycle`, `bound_retry_path_never_reuses_old_admission`. Assert actual handler/transport invocation counts remain zero for every Bound bypass, including a held mount handler after stop/retirement and an inbound fixture message. Legacy positive controls still deliver, retry, mount and health-restart as before. Bound raw get None must not become a security “non-channel” allow rule: no Bound input reaches legacy inbound authorization, because deny-only ingress/forwarder refusal is pinned here. No durable delivery queue row is added by refused Bound helpers.
Core bypass assertions after actual compatibility dispatch attempts:

```rust
assert!(registry.get(&bound_id).await.is_none());
assert!(registry.get(&legacy_id).await.is_some());
assert_eq!(after_bound_attempts.transport_calls, 0);
assert_eq!(after_bound_attempts.tickets_minted, 0);
assert!(legacy_send_result.is_ok());
assert!(legacy_after.transport_calls > 0);
```

For webhook dispatch, the held Bound wrapper's `verify(&HeaderMap, &[u8]) -> bool` always returns false and `handle(&HeaderMap, Bytes) -> ChannelResult<Vec<InboundMessage>>` always returns UnsupportedFeature even if invoked directly; `path() -> &str` only returns the immutable declared path. Do not delegate verification/handle to the real fixture handler. Actual endpoint dispatch therefore rejects before parsing/forwarding, including for a held wrapper after unmount.

- [ ] **Step 3 — Run RED.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_raw_get_and_transport_helpers_fail_closed -- --exact` and `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_webhook_and_forwarder_do_not_dispatch_legacy_ingress -- --exact`; expect an observable bypass failure, not a fabricated fixture assertion.
- [ ] **Step 4 — Complete central dispatch.** Inspect all map iteration paths as well as get-based methods: never leak a Bound ChannelHandle from capture/list/read helpers, never call Bound trait health/directory/network methods during cold reads, and never hand raw channel webhook handlers to the mount table. Metadata/status snapshots use entry-stored declaration/state observations without runtime initialization. Retain the Task 4 source-owned deny-only wrapper and suppressed Bound forwarder delivery until a separately approved ingress operation exists; no generalized invocation envelope. Existing webhook mount table remains a projection, not an authority. Keep concrete legacy handlers, slot wire contracts, existing telecom fences and boot config behavior untouched.
- [ ] **Step 5 — GREEN and compatibility verification.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::`, `cargo test -p alephcore --lib gateway::webhook_receiver::tests::`, `cargo test -p alephcore --lib gateway::delivery_queue::tests::`, `cargo test -p alephcore --lib gateway::channel_health_monitor::tests::`, `cargo test -p alephcore --lib tools::registry::tests::` and `cargo check -p alephcore --tests`. Require nonzero matching test counts for existing filters; a zero-test pass is not evidence. Review confirms there are still no production Bound registrations and no permission downgrade through get None.
- [ ] **Step 6 — Commit.** Suggest `fix(gateway): reject unfenced bound compatibility calls`; stage only the two task files.

### Task 8: Prove deterministic cold/resource matrices with two transport shapes / 双形态资源取证

**Files:**
- Create: none.
- Modify: `src/gateway/channel_registry/test_support.rs`, `src/gateway/channel_registry.rs` (tests only, except fixing defects uncovered in earlier source tasks).
- Test: inline `src/gateway/channel_registry.rs::tests`, using real implementations of the new source APIs.

**Interfaces:**
- Consumes: complete source registration/lifecycle/admission/completion, ResourceTracker/ResourceSnapshot from Task 4, existing Tokio/reqwest/rusqlite. Fixture constructor remains cold.
- Produces two test-only Channel implementations: `WebhookHttpFixture` (HTTP client + loopback token/send requests, shared Gateway webhook shape) and `PollingSocketFixture` (poll worker + interval, loopback TCP connection, provider-owned cache + SQLite database). No channel-type branches in production source logic.
- Tracker records per-resource attempt/started/opened/cancelled/closed/joined/active counters and admission/transport/result/verdict/drain events. Counters change only adjacent to actual construction/open/spawn/request/close/join; never by a test pretending an effect occurred. Loopback peer/server resources are separately labelled test infrastructure, not misreported as provider runtime.

- [ ] **Step 1 — Complete real fixture boundaries before matrix assertions.** Add actual reqwest client build and local token/send request observations to WebhookHttpFixture, no provider-owned listener claim (the existing Gateway owns its listener). PollingSocketFixture opens a loopback TcpStream, starts a real worker and Tokio interval, initializes a provider cache and temporary SQLite connection at start; stop closes/joins/drops them and only then records cleanup. If a dedicated thread is used to own SQLite, retain its JoinHandle and join it; otherwise thread receipts are N/A, never claimed as covered. Fixture errors use loopback refusal/oneshot-controlled failure injection at the actual operation boundary. No external accounts, secrets, internet traffic or Feishu files.
- [ ] **Step 2 — Write RED cold-matrix tests.** Add `cold_read_matrix_webhook_http_has_zero_runtime_attempts`, `cold_read_matrix_polling_socket_has_zero_runtime_attempts`, `lifecycle_descriptor_never_becomes_send_callable`. For each Discovered/Cold/Disabled initial state: register without activation; repeatedly list, source snapshot (describe/project), capture and resolve, unknown ID, wrong visibility and denied direct admission; assert every applicable attempt and active delta is zero. Disabled boot intent never starts it. Legacy reads stay compatible but resolve/admit UnboundLegacy. Compare descriptor schema/full hashes before/after send payload variation; schema/Callable status do not change. A Ready descriptor still has no Callable/send projection, and an operation schema cannot enter the contract constructor.
- [ ] **Step 3 — Write positive/failure controls.** Add `activation_and_send_receipts_cover_both_non_feishu_shapes`, `failed_resource_attempts_are_counted_and_residual_is_honest`, `stop_receipts_require_real_close_drop_cancel_and_join`. Explicit activation creates the applicable client/request/connection/cache/database/task/timer receipts; a real admitted send reaches the transport, yields Accepted or DiscardedStale as orchestrated, then releases drain. Successful stop requires active zero and all applicable joins/closes complete, including registry forwarder. Failed start/connect/auth/client/database initialization increments attempt receipts even when success count stays zero. A cleanup failure exposes Runtime residual matching actual live resources, not a state-derived zero.
Core cold/positive-control assertions are deltas from each fixture instance's pre-operation snapshot; additionally assert each applicable resource row, not merely these totals:

```rust
assert_eq!(after_reads.provider_runtime_attempts - before_reads.provider_runtime_attempts, 0);
assert_eq!(after_reads.active_resources - before_reads.active_resources, 0);
assert_eq!(after_reads.tickets_minted - before_reads.tickets_minted, 0);
assert_eq!(after_reads.transport_calls - before_reads.transport_calls, 0);
assert!(after_activation.provider_runtime_attempts > before_activation.provider_runtime_attempts);
assert!(after_send.transport_calls > before_send.transport_calls);
assert_eq!(after_stop.active_resources, 0);
assert_eq!(after_stop.forwarders_spawned, after_stop.forwarders_joined);
```

- [ ] **Step 4 — Run RED, then finish minimal fixture/source fixes.** Run `cargo test -p alephcore --lib gateway::channel_registry::tests::cold_read_matrix_`, `cargo test -p alephcore --lib gateway::channel_registry::tests::activation_and_send_receipts_cover_both_non_feishu_shapes -- --exact` and `cargo test -p alephcore --lib gateway::channel_registry::tests::failed_resource_attempts_are_counted_and_residual_is_honest -- --exact`; expect missing receipts or specific resource leaks, not false manual counter increments. Fix only the owning boundary; if production logic changes, return it to its worker/reviewer gate before continuing.
- [ ] **Step 5 — GREEN and deterministic repeat.** Re-run those commands and `cargo test -p alephcore --lib gateway::channel_registry::tests::`; repeat each cold-matrix command three times serially, recording exit status and matching test count. Use barriers/oneshots/Notify with lost-wakeup-safe waits, never scheduling sleeps; watchdogs may fail a deadlock but cannot establish order. Every test must leave its own peers/tasks/temp files cleaned up. Reviewer checks a receipt coverage table for each fixture, including explicit N/A thread/listener/websocket rows; TCP does not prove websocket cleanup.
- [ ] **Step 6 — Commit.** Suggest `test(gateway): prove cold reads and resource cleanup`; stage only the two task files. These fixtures prove two common contract shapes, not production constructor coldness or a formal inventory of all social channels.

### Task 9: Verify the implementation evidence, then update references / 验证后更新文档

**Files:**
- Create: none.
- Modify: `docs/reference/FEATURE_LOCATOR.md` (Capability Core/R1 status and source/test anchors), `docs/reference/GATEWAY.md` (canonical binding, compatibility and lifecycle/resource boundaries only).
- Test: existing inline suites in `src/capability/provider.rs`, `src/capability/ownership.rs`, `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`; no new code in this task.
- Inspect only: approved spec, `src/capability/backend.rs`, `src/capability/zahir_facade.rs`, `src/tools/registry.rs`, `src/session/store.rs`, `crates/aleph-slot-api/src/channel.rs`, and actual prior task diffs/results.

**Interfaces:**
- Consumes: Tasks 1–8 reviewed implementation signatures and actual passing results; no design-only assertion becomes an implementation fact.
- Produces: bilingual references to the real source owner, lifecycle-only schema/descriptor, explicit Bound opt-in/internal proof boundaries, failure residuals and verified tests; no new runtime API. R1 gate ruling remains separate, never automatically GREEN.

- [ ] **Step 1 — Test first, before writing documentation.** This documentation-only task has no invented RED code change. Re-run all predecessor focused suites; a failure blocks documentation completion and is returned to the owning worker. Verify every promised test exists, runs with a nonzero match count, and asserts actual effects. The source-provenance, owner same-nonce replacement, stop/completion, stale successor and both cold matrices must be present and GREEN.
- [ ] **Step 2 — Run the required final commands serially from repository root.** Before **each cargo** command run `python qa/browser_live/check_memory.py` and require exit 0. Execute the following commands exactly; preserve failures and output summaries rather than hiding them with a successful pipe:

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib gateway::channel_registry::tests::
```

Also run `cargo test -p alephcore --lib gateway::channel_registry::binding::tests::` and `cargo test -p alephcore --lib tools::registry::tests::`, since the required registry filter does not include the sibling binding tests and Tool proof compatibility must be checked. These commands do not replace broader project verification when affected surfaces expand; no Panel/desktop/shared-API surface is planned here. Existing unrelated failures are blockers to a GREEN claim, not permission to edit outside task scope.
- [ ] **Step 3 — Independent whole-diff review.** A fresh reviewer checks the final branch/task diff against the approved spec and this plan, not worker summaries alone. Explicitly check no independent fake ProviderHandle, no second association map, no raw Bound effect bypass, no callback I/O/await, correct full ownership binding and no unjoined forwarder. Verify there is still no production Zahir mount/route, SessionEventStore change, slot dependency inversion, send Callable/schema or Feishu/R2 code. Return defects to the owning worker and repeat the affected tests before documentation.
- [ ] **Step 4 — Update the two references only after evidence exists.** Link actual source/test anchors, actual commands/results and known unhandled resources; distinguish lifecycle descriptor identity/schema from future operation identity/schema. Describe Legacy admission rejection and Bound deny-only ingress/compatibility limits. State that Tool remains the only mounted callable backend, channel production mount and full all-channel inventory are deferred, and source-local acceptance/drain are not durable recovery or external exactly-once. Do not edit the approved spec, earlier capture ledger/roadmap or constitutional entry files as part of this documentation task.
- [ ] **Step 5 — Final verification and commit.** Re-run `git diff --check` after reference edits, inspect only intended documentation hunks and record the exact prior verification commands/statuses in the handoff (do not create an extra report file). Suggest `docs(capability): record channel binding implementation evidence`; stage only the two reference files. Final delivery lists completed tasks, actual verification, residual limitations and explicit non-work: no real-channel migration, no production ingress/send operation, no Feishu/R2, no durable/exactly-once/replay guarantee, no automatic R1 GREEN ruling.

## Plan Completion Criteria / 计划验收

- Each task has a concrete producer/consumer boundary and a RED→GREEN cycle (documentation validates existing evidence first), with independent review before dependency handoff.
- Source provenance starts at actual Bound registration and reaches actual lifecycle/transport/completion; Legacy has no inferred provider. Four independent domains remain separate: source instance/publication, provider instance/generation, OwnershipTree binding/generation, and untouched SessionEvent ordering.
- Ordinary stop accepts genuine pre-stop completions while draining; retirement rejects them and settles only original work. Replacement/remove failures retain incumbent identity and honest residuals; publication/restart exhaustion never wraps or half-publishes.
- Both non-Feishu fixtures prove applicable resource attempts stay zero for disabled/discovered/cold reads and grow at real activation/send boundaries, then close/join on successful cleanup. Uncovered resource kinds remain explicit N/A, not inferred coverage.
- All final commands are executed and reported during implementation, not claimed by this planning session. Only then may reference docs describe implementation evidence; production mount, future operation contracts and a separate R1 gate decision remain outside this plan.
