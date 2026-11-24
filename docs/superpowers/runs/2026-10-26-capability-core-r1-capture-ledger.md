# SDD ledger — R1 source-owned capture slice

Plan: `docs/superpowers/plans/2026-10-25-capability-core-remaining-roadmap.md`
Spec: `docs/superpowers/specs/2026-10-26-r1-provider-source-lease-design.md`
Date: 2026-10-26

## Scope and gate status

This ledger records one bounded R1 slice after the common `ChannelRegistry` teardown slice. It does not authorize R2, Feishu wiring, a production mount, a Zahir route, a universal lease, or any EffectClaim/recovery work.

**Decision:** bounded source-owned capture slice PASS; overall R1 remains **NO-GO / deferred**.

## Authority boundary

| Concern | Existing authority | Slice result |
|---|---|---|
| Channel instance and lifecycle | `src/gateway/channel_registry.rs:187` `ChannelRegistry` | Remains sole owner of channel map, desired intent, lifecycle calls, and webhook mounts |
| Source publication revision | `ChannelRegistry` | Added `AtomicU64` revision, advanced only after committed registration/unregistration map mutation |
| Source capture | `ChannelRegistry` | Added crate-private `ChannelCapture`; stores channel id, source revision, and the registered `Arc` identity |
| Capture validation | `ChannelRegistry` | Added `validate_channel_capture`; compares current and captured channel handles with `Arc::ptr_eq` under the registry lifecycle boundary |
| Provider lifecycle/generation | `src/capability/provider.rs:234` `ProviderHandle` | Unchanged; not copied into `ChannelCapture` |
| Visibility/owner generation | `src/capability/ownership.rs:334` `OwnershipTree` | Unchanged; not copied into `ChannelCapture` |
| Descriptor/handler source | `src/tools/registry.rs:1519` `ToolHandlerRegistry` for Tool | Unchanged; no non-Tool descriptor registry added |
| Durable recovery | `src/session/store.rs:56` `SessionEventStore` | Unchanged; live capture is not recovery evidence |

No second callable registry, descriptor registry, handler map, enabled-state map, owner/generation map, disposer map, or durable store was introduced.

## TDD evidence

### RED

Added three tests before implementation in `src/gateway/channel_registry.rs`:

- `capture_is_read_only_and_unrelated_publication_keeps_it_valid`
- `capture_rejects_replaced_channel_instance`
- `capture_rejects_unregistered_channel`

The focused command failed to compile with the expected missing API errors:

- `no method named capture_channel found for struct channel_registry::ChannelRegistry`
- `no method named validate_channel_capture found for struct channel_registry::ChannelRegistry`

An initial test insertion temporarily removed the replacement test declaration and produced an unexpected closing delimiter. The declaration was restored before the meaningful RED run; that parser error is not implementation evidence.

### GREEN

Implementation in `src/gateway/channel_registry.rs`:

- `ChannelCapture` is crate-private and cloneable.
- `capture_channel(&self, channel_id: &ChannelId) -> Option<ChannelCapture>` takes the lifecycle mutex, reads the channel map, and captures the current `Arc` and source revision without calling `start`, auth, transport, or network code.
- `validate_channel_capture(&self, capture: &ChannelCapture) -> Option<ChannelHandle>` takes the same lifecycle boundary and rejects missing/replaced entries through `Arc::ptr_eq`.
- `source_revision` advances only after registration or unregistration has mutated the canonical channel map.
- `AtomicU64::try_update` is used with checked increment; exhaustion fails loudly instead of reusing a revision.

Focused result:

```text
running 3 tests
... ok

 test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 21460 filtered out
```

The test binary emitted the existing macOS linker warning:

```text
ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table, performance of exception handling might be affected
```

This warning did not change the exit status.

## What this proves

- Capture is read-only with respect to channel lifecycle: a captured `WebhookyChannel` remains `Disconnected`.
- A publication of an unrelated channel advances source revision but leaves the first capture valid.
- Replacing the same textual channel id changes the registered `Arc` identity and rejects the old capture.
- Unregister removes the canonical map entry and rejects the old capture.
- The source identity and source revision are kept in the existing `ChannelRegistry`; no parallel source authority exists.

## What remains open

- Binding `ChannelCapture` to the actual `ProviderHandle` instance and provider generation.
- Binding descriptor identity/schema/revision to the same source publication without a second descriptor map.
- Carrying `OwnershipTree` owner generation as a separate observation.
- Defining admitted-work ownership, drain, completion acceptance, and late-completion rejection.
- Resource receipts proving disabled/discovered read paths do not initialize threads, connections, databases, caches, token clients, listeners, or websockets.

These open items prevent R1 GO. R2 Feishu A/B audit and all Feishu implementation remain deferred.

- The full `gateway::channel_registry::tests::` filter passed: **44 passed, 0 failed, 21,419 filtered out**.
- `cargo fmt --all --check` passed.
- `git diff --check` passed.
- `cargo check -p alephcore --tests` passed.
- `cargo clippy -p alephcore --tests -- -D warnings` passed.
- `src/harness/**` was not modified.
- The test binary emitted the existing macOS linker warning:

```text
ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table, performance of exception handling might be affected
```

The warning did not change any exit status. The one combined check-plus-test attempt exceeded the harness timeout during test linking; the check and test were then run separately and passed.

## Independent review note

The read-only review found no capture-specific lock-order, revision, stale-identity, or authority-duplication issue. It flagged that failed `stop()` retains the incumbent map entry/webhook surface while `stop_channel_inner` has already set `DesiredChannelState::Stopped`. This is intentional fail-safe behavior: the adapter has already received a stop request, so the health monitor must not race in with an automatic restart after a partial stop failure. The bounded teardown contract says the incumbent registration and runtime surface are retained; it does not promise to retain a restart intent. No change was made in this capture slice. A future lifecycle contract may revisit partial-stop recovery with explicit drain evidence.

## Provider binding completion probe — 2026-10-26

**Decision:** R1 remains **NO-GO / deferred**. The bounded source capture is proven, but the first remaining contract is still source-owned provider binding.

### RED evidence

The probe was inserted in the real `src/gateway/channel_registry.rs` test module, after correcting an accidental test declaration removal that caused a parser error. The focused command was:

```text
cargo test -p alephcore --lib gateway::channel_registry::tests::capture_exposes_provider_instance_and_generation_for_admission -- --exact
```

The historical RED was a production API absence, not a malformed test. Task 1 replaced the invalid independent-provider probe with source-backed cold binding tests; the current tree does not retain a deliberately failing test.

The historical evidence recorded that the existing Gateway capture could not expose or validate a real provider instance/generation. No speculative provider field, registry, adapter, or facade glue was kept as a substitute. Task 1 now publishes a source-minted provider in the canonical Bound entry; Tasks 2 and 3 add provider-domain and owner-domain guards through their existing authorities.

### Authority result

- `ChannelRegistry` remains the sole owner of channel instances and source publication; the new Bound capture is published from its canonical entry and remains distinct from legacy capture.
- `ProviderHandle` owns provider lifecycle, instance identity, generation, and the Task 2 local guards; it does not own a Gateway work ticket or drain.
- `OwnershipTree` independently validates owner generation, visibility, owner/lifetime, revoke, and dispose through the Task 3 guard; it does not become a second owner map.
- Descriptor identity/schema/revision and post-await completion validation exist in the Tool-owned `ToolHandlerRegistry`/`ZahirFacade` path. They are not a canonical source for Gateway channels, and copying them into a new channel registry would violate the authority table.
- No existing Gateway/provider owner owns admitted work, drain, completion acceptance, or resource receipts. The current guards are independently tested primitives, not a Gateway completion proof.

The existing Tool tests remain useful for their own authorities, but they do not establish a non-Tool admitted-work path. Therefore no simulated completion fence, synthetic ticket, detached drain counter, or receipt registry was implemented; R1 stops at the remaining contract.

### Validation run — 2026-10-26

- `cargo fmt --all --check` — PASS.
- `git diff --check` — PASS.
- `cargo check -p alephcore --tests` — PASS.
- `cargo clippy -p alephcore --tests -- -D warnings` — PASS.
- `cargo test -p alephcore --lib capability::` — PASS, 222 passed / 0 failed.
- `cargo test -p alephcore --lib gateway::channel_registry::tests::` — PASS, 49 passed / 0 failed.

The initial Task 1 binding test invocation reached the 300-second harness timeout while compiling; the same focused binding suite was rerun with a 900-second limit and passed 3/3. The existing linker warning `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table` was non-fatal. No current deliberately failing test remains in the tree; the historical RED is preserved in this ledger and the design record only.

### Next gate

Keep the bounded capture slice PASS and overall R1 NO-GO. The only next entry is to revise the source-owned capture contract with a concrete existing provider/source owner and a real admission/completion boundary. Do not enter R2 Feishu or add a registry, adapter, facade, mount, universal lease, EffectClaim producer, or durable recovery path.

## Remaining gate blockers

- Bind `ChannelCapture` to the actual `ProviderHandle` instance and provider generation.
- Bind descriptor identity/schema/revision to the same source publication without a second descriptor map.
- Carry `OwnershipTree` owner generation as a separate observation.
- Define admitted-work ownership, drain, completion acceptance, and late-completion rejection.
- Produce resource receipts proving disabled/discovered read paths do not initialize runtime resources.

These blockers keep overall R1 NO-GO/deferred. R2 Feishu A/B audit and all Feishu implementation remain deferred.

## Revision continuation result — 2026-10-26

Tasks 1–3 of the approved source-owned provider binding revision are complete and independently reviewed:

- `12ff9fb3f` and `914d4e974`: canonical cold `ChannelRegistry` binding, source-minted `ProviderHandle`, lifecycle-only descriptor/schema fingerprints, source/entry identity, and Bound legacy fail-closed behavior; the follow-up fixed the large enum and removed the module-level `dead_code` suppression.
- `5fde0b3f8`: existing `ProviderHandle` admission/completion guard primitives, with generation exhaustion and Ready/Stopping matrix coverage; no Gateway work ticket or drain was added.
- `d293f8676`: existing `OwnershipTree` exact binding validation for visibility, owner/lifetime, independent owner generation, revoke/dispose, and same-nonce replacement; no owner map was added.

Recorded evidence: binding 3/3; registry 49/49; provider 10/10; ownership 30/30; capability 222/222; Tool registry 54/54; `cargo fmt --all --check`; `cargo check -p alephcore --tests`; and `cargo clippy -p alephcore --tests -- -D warnings` passed at the completed gates. The existing macOS linker `__eh_frame` warning was non-fatal. The first 300-second binding test invocation timed out during compilation and was rerun with a 900-second limit, passing 3/3.

**Stop condition reached:** no existing Gateway/source authority owns admitted work from a real send boundary through a non-cloneable proof, completion acceptance at a real linearization point, or deterministic receipts at thread/connection/database/cache/token-client/listener/websocket allocation boundaries. Therefore Tasks 4–9 were not implemented. The precise unresolved contract is Gateway admitted-work/completion acceptance plus real resource receipts; Tool-specific provider/owner/completion tests do not substitute for it.

R1 remains **NO-GO / deferred**. R2 Feishu, production mount, Zahir route, EffectClaim producer, Safe Replay, external exactly-once, universal scheduler, and durable recovery were not executed. Any future continuation must first identify an existing owner for the missing facts; even a later R1 GO requires a separate R2 Feishu A/B re-gate.
