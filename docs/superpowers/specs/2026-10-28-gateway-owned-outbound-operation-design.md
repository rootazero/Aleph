# Gateway Owned Outbound Operation — Design / 设计

**Status / 状态:** APPROVED by the human on 2026-10-28. Implementation follows the separate plan. / 已于 2026-10-28 获人工批准；实施遵循单独计划。

**Date / 日期:** 2026-10-28 · **Branch:** `mid-send-closure` · **Base:** `main` @ `3ac50c1be5c3a613195cb76c080ef7d93f99d9ba`

## 1. Intent and why / 意图与原因

Stage C left two executable RED contracts in `tests/sms_delivery_policy_test.rs`:

- `explicit_only_channel_unregistered_mid_send_is_never_persisted`
- `explicit_only_channel_replaced_mid_send_is_never_persisted`

Both deadlock identically on current `main` and on pre-Stage-C baseline `ca03a1206` (12/12 original runs exit 101, `no deadlock: Elapsed(())` at 5.00–5.01s; evidence in `.superpowers/sdd/2026-10-28-gateway-stage-c-verification-closure-plan/`). The wait cycle is structural, not a Stage C regression:

```text
ChannelRegistry::send_on            src/gateway/channel_registry.rs:1618
  holds channel.read() across
  channel.send(message).await       src/gateway/channel_registry.rs:1653
    → VanishingExplicit::send awaits registry.unregister()
      → stop_outgoing_channel → stop_channel_inner
        awaits channel.write()      src/gateway/channel_registry.rs:1227
          = write-after-read on the same RwLock, same task chain
```

Because `Channel::send(&self)` returns a future that borrows the adapter, the registry must hold the read guard for the whole transport call. Any send that synchronously manages its own registration therefore self-deadlocks, cleanup never runs, and cancellation of the outer future strands the lifecycle operation marker (post-timeout `stop_channel` returns `Busy("Channel lifecycle operation already active: vanish")` on both revisions).

The chosen direction (human-approved 2026-10-28, approach A of `approaches-under-review.md`): make an outbound send an **owned operation**. Preparing the operation happens under the short-lived instance read guard together with policy capture; driving the transport happens after the guard is released, on a future that owns everything it needs (`'static`, no borrow of the adapter).

## 2. Goals / 目标

- G1: Both named mid-send tests retain their existing assertions and 5-second watchdogs, with added explicit proof required by the closure contract — unregister case proves old registration removed **and** old instance `stop` completed; replacement case proves Automatic successor usable, old ExplicitOnly instance sent exactly once and was stopped, and neither case persists the old payload (queue rows and dead letters stay empty).
- G2: The delivery policy used for enqueue/replay decisions is always captured from the **actual sending instance**, never from a same-ID successor and never from a vanished registration.
- G3: `unregister`/replacement return only after the old instance's `stop` has actually completed; no detached cleanup, no fake receipts, no manual active-work permission grants.
- G4: No `unsafe`, no self-referencing guards, no second association/owner/cleanup authority. `ChannelRegistry` stays the sole Gateway association authority.
- G5: All existing Stage C required suites stay GREEN (Registry, Webhook receiver, delivery queue, health monitor, tools registry, Webhook interface, capability) plus `cargo check -p alephcore --tests`, strict clippy, fmt.

## 3. Non-goals / 非目标

- No Stage D resource matrices, no test-only polling/socket providers, no Feishu/R2 work, no production Capability mount.
- No change to `start(&mut self)` / `stop(&mut self)` signatures, no change to `OwnershipTree`, `ProviderHandle`, `EntryRuntime` responsibilities.
- No change to public `unregister` semantics (still stop-then-remove, synchronous cleanup).
- No general fix for providers that hold their *own* internal lock across a transport await; that remains a provider-local contract (§7).
- No new deferred-cleanup authority, no background "reaper" task.

## 4. Anti-goals / 反目标

- Releasing the read guard while a `&self`-borrowing future still exists (unsound; does not compile for good reason).
- An owned future combined with stop draining in-flight sends — that re-creates the same cycle one level down (operation awaits unregister awaits stop awaits the operation).
- Cloning `Arc<RwLock<Box<dyn Channel>>>` into the send path and calling that "owned" — the borrow, not the handle, is the problem.
- Making the new API an opt-in second mode beside the borrowed `send`: two supported send modes would leave the Legacy path non-reentrant forever and split every provider's contract. Exactly one outbound API shape is delivered.
- Silently rewriting the two RED tests to external-task retirement (that was approach C, explicitly not chosen).

## 5. Constraints / 约束

- `Channel` is the pure public slot contract (`crates/aleph-slot-api/src/channel.rs`, re-exported at `src/gateway/channel.rs`); changing its `send` signature is a breaking public API change, authorized in principle by the human's approach-A decision and by instruction 9's architecture gate (this document is that gate's artifact).
- `lifecycle_sync` and the source publication gate protect only short identity/marker/publication commits; channel/transport/stop/close/join I/O stays outside them (unchanged).
- Every Cargo invocation is individually preceded by `python qa/browser_live/check_memory.py` requiring exit 0 / `ALLOW`.
- No test deletion, weakening, or longer watchdogs. The existing 5-second watchdogs stay.
- 中文协作、英文代码注释、双语文档；`cargo` 验证以实际 exit code 为准。

## 6. Approaches considered / 方案比较

| Approach | Summary | Verdict |
|---|---|---|
| **A — owned outbound operation (chosen)** | Prepare under the read guard, drive a `'static` future after releasing it. Public `send` signature change + provider migration. | Directly separates an in-flight request from mutable adapter ownership; preserves the exact self-reentrant tests. Chosen by the human. |
| B — shared lifecycle methods | `stop(&self)` + provider interior state, stable instance handles. | Larger provider synchronization migration; still deadlocks if stop waits for its own send. No demonstrated advantage. |
| C — amend self-reentrancy contract | Forbid send→unregister reentrancy, rewrite tests to external retirement. | Changes the required contract; rejected by the human's choice of A. |

Trade-off A accepts and must state plainly: after this change, **`stop` no longer implies "no sends are in flight"**. It means "the provider's own resources are closed and no new operations can be prepared"; an already-prepared operation holds only operation-scoped clones and may finish (usually with an error) after stop returns. See §7 — this is the deliberate semantic price of breaking the cycle, and it makes per-provider resource auditing harder than the old borrowed model did.

## 7. The design / 设计内容

### 7.1 API shape (slot-api)

One outbound method, replacing the borrowed async `send`:

```rust
/// An owned outbound operation: prepared under the registry's instance
/// read guard, driven after that guard is released. Owns every handle it
/// needs; never borrows the adapter.
pub type SendOperation =
    Pin<Box<dyn Future<Output = ChannelResult<SendResult>> + Send + 'static>>;

#[async_trait]
pub trait Channel: Send + Sync {
    // ... unchanged members ...

    /// Prepare an owned send. Synchronous: clones whatever the transport
    /// needs and returns the operation. The returned future must not
    /// borrow `self` and must not require `stop` to drain it.
    fn send(&self, message: OutboundMessage) -> SendOperation;

    /// Stop closes provider-owned resources. It must NOT wait for
    /// in-flight SendOperations: an operation may synchronously manage
    /// its own registration (send → unregister → stop).
    async fn stop(&mut self) -> ChannelResult<()>;
}
```

- `SendOperation` is exported from `aleph-slot-api` and re-exported through `src/gateway/channel.rs` alongside `Channel`.
- `async fn send` is removed, not kept as a default alongside the new method (single API shape, §4). `async_trait` remains for the other async members.
- Object safety: `fn send(&self) -> SendOperation` is object-safe; `Box<dyn Channel>` is unchanged, so registry storage, `get`, and all non-send call sites are untouched.

### 7.2 Registry `send_on` restructure

Current structure per attempt: acquire read guard → capture policy → replay/disabled validation → `channel.send(...).await` under the guard → drop guard.

New structure per attempt:

1. Acquire `channel_arc.read().await`.
2. Capture `policy = channel.outbound_delivery_policy()` (unchanged, from the actual instance).
3. Run the existing replay-refusal, `begin_replay`, and `Disabled` validations under the same guard (unchanged logic and error values).
4. Prepare `let operation = channel.send(message.clone());` — synchronous, no await.
5. `drop(channel)` — the guard is now released **before** any transport or reentrant lifecycle await.
6. `let result = operation.await;` — transport runs guard-free.
7. Rate-limit retry loop, observer hooks, and `Attempt { result, policy, replay_refused }` construction are unchanged; each retry re-acquires the guard and revalidates exactly as today.

`send_attempt`, `send_reported`, `maybe_enqueue`, and the delivery-queue replay caller need no signature or policy-flow changes: `Attempt.policy` still originates at step 2 on the actual sending instance.

### 7.2a Every other registry-owned `Channel::send` path / 其余注册表发送路径

The same prepare-under-read / drive-after-drop pattern applies to **all** dynamic `Channel::send` call sites, not only `send_on`; leaving one borrowed await would preserve the same reentrancy hazard:

- `send_public_telecom` (`src/gateway/channel_registry.rs:1292-1299`): prepare the operation under the read guard, drop the guard, then await it. Its bypass of observers, persistence, queues and retries is unchanged.
- `send_public_telecom_if_not_cancelled` (`src/gateway/channel_registry.rs:1309-1334`): retain every cancellable registry/lock wait and every existing cancellation fence. After the final fence, synchronously prepare the operation and drop the guard, then immediately await it with no intervening cancellation select or other await. **Preparation is not transport commitment: the operation's first poll remains the commit boundary**, so no await is introduced between that first poll and the final fence.
- `send_bound_once` (`src/gateway/channel_registry/binding.rs:410-413`): inside the already-admitted execution task, prepare under the capture's read guard, drop it, then await. Owner/generation/call-admission checks, bounded execution slots, `active_work` accounting, and `complete_bound_work` are unchanged; no manual active-work permission is introduced.
- `send_approved_public_telecom` uses the separate `send_private_sms` pinned-transport contract, not `Channel::send`; it is not changed by this migration.

### 7.2b Required fixture proof / 必须补强的夹具断言

Builder check confirmed that the existing tests do not themselves observe stop completion or successor usability. The migration therefore adds proof without deleting or weakening any existing assertion:

- Add a stop counter to the test-only `Sends` fixture state and increment it in `VanishingExplicit::stop`.
- Unregister case: after the original `NotConnected` / exactly-one-send / nothing-stored assertions, assert `registry.get("vanish").await` is `None` and the old stop count is exactly 1.
- Replacement case: assert the old stop count is exactly 1 and the old send count remains 1; obtain the same-ID successor and assert its `outbound_delivery_policy()` is `Automatic`, dropping that read guard before further registry calls; send a distinct non-secret message through the registry and assert success. Assert nothing was stored both before and after that successor send. The old `"mid-send-secret"` payload must never appear in queue rows or dead letters.
- The tests still use self-reentrant `send → unregister[/register]`, retain their 5-second watchdog, and keep all original assertions. No external-task retirement rewrite.

### 7.3 Stop / unregister semantics (explicit contract)

- `stop_channel_inner` still takes the channel write lock and awaits `stop(&mut self)`; with the read guard no longer held across transport, a self-reentrant `send → unregister → stop` chain can acquire it.
- Provider contract for `stop`: close provider-owned connections/tasks/listeners; do **not** join or drain in-flight `SendOperation`s. Operation-scoped clones (HTTP clients, tokens, sockets created for that request) are the operation's resources, not the provider's; their natural completion or failure after stop is expected and is not "uncleaned provider state".
- `unregister` and replacement keep their existing ordering: successful `stop` → unmount → drop desired state → remove entry / publish successor. No return-before-cleanup, no detached tasks.
- Marker lifecycle is unchanged (`begin_legacy_operation` … `finish_legacy_operation`); with the cycle removed, the stranded-marker state observed after watchdog cancellation no longer arises in these flows.

### 7.4 Provider migration (mechanical, all implementors)

Every `impl Channel` migrates from `async fn send(&self, ...)` to `fn send(&self, ...) -> SendOperation`, moving its existing body into `Box::pin(async move { ... })` after cloning the handles it uses. A full `rg 'impl Channel for'` on the base revision finds **36 files**, so the migration is repo-wide, not interfaces-only:

- 21 interface adapters under `src/gateway/interfaces/`: cli, discord, email, feishu, imessage (+ bluebubbles), irc, line, matrix, mattermost, nostr, qq, signal, slack, sms, telegram, voice_call, webhook, wechat, whatsapp, xmpp. **The existing feishu adapter's signature migration is mechanical API adaptation only — it is not Feishu/R2 wiring, feature work, or Stage D authorization.**
- Other in-repo implementors: `src/clarification/ask.rs`, `src/exec/approval/channel_bridge.rs`, `src/gateway/channel.rs`, `src/gateway/event_emitter/origin_fanout.rs`, `src/gateway/execution_engine/run_trace_sinks.rs`, `src/gateway/inbound_router/executor.rs`, `src/gateway/resume_coordinator.rs`.
- `examples/modularization/channel/src/lib.rs` (ExampleChannel) and slot-api documentation examples referencing `async fn send`.
- Test doubles and fixtures: `tests/sms_delivery_policy_test.rs` (MockChannel, VanishingExplicit), `tests/link_acl_probe/mock_channel.rs`, `src/gateway/channel_registry.rs` and `src/gateway/channel_registry/test_support.rs`, `src/gateway/reply_emitter/tests.rs`, `crates/aleph-slot-api/tests/channel_compat.rs`.

The builder re-runs the search at implementation time; any implementor added since the base revision migrates too.

`VanishingExplicit` migrates by cloning the registry handle and channel id into the returned operation; its attempt counter increments synchronously in `send` (preparation), preserving "old instance sent exactly once". The two RED tests' self-reentrant structure, original assertions, and watchdogs are untouched; §7.2b specifies only the additional cleanup/removal/successor assertions required by G1.

### 7.5 What the builder may decide / 留给实现者

- Exact placement of the `SendOperation` alias and its doc wording; whether a small helper macro is warranted for the `Box::pin` boilerplate (default: no macro).
- Per-provider clone granularity (whole client struct vs individual fields), using the cheapest existing `Clone` handles.
- Mechanical formatting/import adjustments, and any test-fixture constructor tweaks needed by the migration that do not alter assertions.

## 8. Verification plan / 验证计划

1. Pre-change baseline already recorded (12 original RED runs + 4 diagnostic runs, all memory-gated). No new RED needed: the two existing tests are the executable RED.
2. After implementation, each focused mid-send test runs **three times serially**, with real exit codes and passed/failed/ignored counts logged; watchdog stays 5s.
3. Full required set, each individually memory-gated:

```bash
cargo test -p alephcore --test sms_delivery_policy_test
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib capability::
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo fmt --all --check
git diff --check
```

4. Independent read-only review of the fix plus the complete Stage C diff (lock cycles, cleanup-before-return, per-instance policy, plaintext queue/dead-letter/replay leaks, Bound denial / cold read / source lifecycle / Legacy positive controls, authority duplication, deferred-subsystem scope). If no reviewer model is available, record the full error and perform the same checklist manually without claiming independent passage.
5. Only after all suites GREEN and no unresolved Critical/Important findings: update the bilingual run record, source plan, and `FEATURE_LOCATOR.md`. Stage C stays open until then.

## 9. Risks and open trade-offs / 风险与权衡

- **R1 (semantic):** in-flight operations may outlive `stop` (§6, §7.3). A provider whose `stop` invalidates shared state an operation still uses will surface an operation error, not a deadlock; this is accepted but must be documented per provider during migration.
- **R2 (breaking API):** downstream/out-of-tree `Channel` implementors break at compile time. Accepted as part of approach A; release notes must call it out.
- **R3 (provider-local locks):** a provider that holds its own mutex across a reentrant await can still self-deadlock. Migration includes a per-provider check for that pattern; fixing such a provider's internal locking is in scope only where the migration touches it.
- **R4 (scope creep):** any discovered need for deferred-cleanup authority, changed public unregister semantics, or further lifecycle API changes stops work and returns to the human per instruction 9.

## 10. Builder check resolution / Builder 检查结论

An independent read-only builder check (`del_mvrknlzh_o3dj`) returned two questions. Both are answered by the existing task contract and are now incorporated without another human-question round:

1. **All send paths or only `send_on`?** All four dynamic `Channel::send` paths migrate; the telecom cancellation fence and Bound admission/completion contracts are preserved (§7.2a). The separate pinned private-SMS method is unchanged.
2. **Are G1 guarantees explicit assertions?** Yes. The user already required cleanup completion, removal/successor usability, exactly one old send, and no persistence; the builder may not treat fixture compilation plus the old assertions as sufficient (§7.2b).

The reviewer produced a usable two-question report before its process exited SIGTERM. This was a design builder check, **not** the required final independent Stage C implementation review.
