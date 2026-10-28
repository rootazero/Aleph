# Gateway Bound Compatibility Closure — Stage C Design

日期 / Date: 2026-10-27

状态 / Status: 待用户批准 / Awaiting user approval

## 1. Intent / 意图

Stage C closes the existing Gateway Legacy compatibility surfaces around a source-owned Bound channel. The behavior being protected is narrow: a Bound registration must not obtain execution, resource initialization, queue admission, webhook delivery, or lifecycle authority by reaching an older Legacy surface.

本阶段只闭合现有 Gateway 的 Legacy compatibility surfaces。要证明的行为很窄：Bound registration 不能通过旧 Legacy surface 获得执行权限、资源初始化、队列写入、webhook 入站投递或 lifecycle authority。

The implementation will use the existing `ChannelRegistry`, entry-local `EntryRuntime`, `ProviderHandle`, and `OwnershipTree`. `ChannelRegistry` remains the only Gateway association/source authority. R1 remains NO-GO/deferred until later resource evidence is complete.

## 2. Goals / 目标

- Route Bound compatibility lifecycle calls through the existing source-owned Bound lifecycle operations.
- Reject Legacy raw handles and effect helpers before ticket, transport, queue, media-custody, or external-effect creation.
- Make retained webhook mounts fail closed against source retirement, replacement, or Bound registration before handler dispatch or unified Legacy inbound delivery.
- Prove cold `list`, `status`, `health`, directory, snapshot, and capture-like reads do not start transport, build clients, spawn forwarders, or activate providers.
- Preserve existing Legacy start/stop/restart/send/mount behavior.
- Add five behavioral contract tests using real loopback, webhook, queue/effect, provider/entry/forwarder, and source-publication observations.
- Record exact verification, review availability, deferred scope, and the first contract that remains unclosed.

## 3. Non-goals and anti-goals / 非目标与反目标

This design does not add a public Bound API, production Bound registration, production webhook mount, Zahir route, public Bound send or ingress, EventSource/Subscription, EffectClaim producer, Safe Replay, durable recovery, universal scheduler, new `CapabilityKind`, generalized invocation envelope, second registry, owner map, descriptor map, runtime table, Stage D polling/socket resource matrix, Feishu/R2 work, or `src/harness/**` changes.

本设计不添加 public Bound API、production Bound registration、production webhook mount、Zahir route、public Bound send/ingress、EventSource/Subscription、EffectClaim producer、Safe Replay、durable recovery、universal scheduler、新 `CapabilityKind`、generalized invocation envelope、第二 registry、owner map、descriptor map、runtime table、Stage D polling/socket resource matrix、Feishu/R2 工作或 `src/harness/**` 改动。

The design must not hide a missing source decision behind `Option`, `bool`, a fallback Legacy handle, or a success-shaped no-op. It must not hold `lifecycle_sync` across channel, transport, stop, close, join, or webhook I/O.

## 4. Current state and census / 当前状态与 census

- `ChannelRegistry` owns the canonical channel map, registration variant, source publication revision, and entry-local lifecycle bookkeeping. `ProviderHandle` owns provider instance/state/generation. `OwnershipTree` owns owner/lifetime/generation/revoke/dispose.
- `legacy_channel`, `legacy_entry`, and `reject_bound_legacy_access` already refuse Bound raw access in `src/gateway/channel_registry.rs:848-888`. The remaining lifecycle gap is that `start_channel`, `stop_channel`, and `restart_channel` return `UnsupportedFeature` for Bound instead of using the existing source-owned methods in `src/gateway/channel_registry/binding.rs`.
- `list`, `status_summary`, and `health_states` read channel metadata/state. They have no explicit `start` call, but the contract needs real `ResourceProbe` evidence that a cold Bound declaration does not build a Webhook client, issue a request, spawn a forwarder, or activate a provider.
- `list_conversations` and message helpers (`send`, `react`, `edit`, `send_typing`, `create_poll`, `set_group_icon`) use Legacy resolution. They must remain Legacy-only and refuse Bound before the adapter call.
- `handlers/channel.rs` start performs configuration reads, secret injection, factory construction, registration/replacement, and lifecycle start. Bound rejection must precede those effects. Discord panel list/audit paths construct an HTTP client after a connected-status check and must not treat a Bound target as a compatible Legacy channel.
- `WebhookMount` currently retains handler, inbound sender, status, and channel id. `webhook_endpoint` performs lookup, signature verification, status read, handler dispatch, and inbound forwarding. Status is not source identity and is insufficient after retirement or replacement.
- `DeliveryStore::enqueue` is a raw public legacy/import seam without a Registry/source argument. `send_reported` already rejects Bound before queue flush, `send_attempt`, media custody, and `enqueue_fresh`; replay reaches the registry choke point. The raw seam must not remain a production dynamic-send admission path.
- Health sweep reads `health_states` and calls `restart_channel`. A central lifecycle dispatch must prevent Legacy restart I/O for Bound and preserve desired state, Busy, residual, and Disabled decisions.
- `HeldWebhook`, `LoopbackPeer`, and `ResourceProbe` in `src/gateway/channel_registry/test_support.rs` observe actual lifecycle, client build/drop, request arrival, and cleanup boundaries. They are test observations only, never permission state.

## 5. Approaches considered / 已比较的路径

### A — Registry central compatibility shim with source-aware webhook and queue boundaries (selected)

Existing Registry compatibility entry points dispatch by the canonical registration variant. Legacy calls keep their current admission and I/O behavior. Bound lifecycle calls invoke the existing `start_bound_channel` and `stop_bound_channel` operations, await their existing driver result, and implement Bound restart as source-owned stop followed by source-owned start. A failed stop or start leaves the source in its honest failed/stopped state; it never fabricates Ready.

The webhook mount remains owned by the existing `ChannelRegistry` mount table. Dispatch obtains a short source/registration validation from that owner before handler execution and inbound forwarding. The validation uses the existing canonical publication identity/revision and current registration, without adding an owner map or a second descriptor. A retained handler is therefore unusable after source retirement, replacement, or conversion to Bound.

Dynamic queue admission remains Registry-owned: normal fresh sends and replays continue through `send_reported`, `maybe_enqueue`, and `send_attempt`. The raw `DeliveryStore::enqueue` operation is reduced to an internal legacy/import fixture seam or otherwise requires an already-validated Legacy context; it cannot authorize a dynamic send by channel id alone. The change must preserve existing queue tests and Legacy positive controls.

This is selected because every dynamic decision returns to the sole canonical source and because it closes the two current bypass shapes—retained webhook state and a Registry-free queue write—before their side effects.

### B — Independent guards at every caller

Each handler, monitor, webhook, queue, inbound, and tool caller would perform its own Bound check. This is smaller initially but duplicates the registration predicate and cannot fully validate a retained webhook mount or a raw queue call that has no source identity. It leaves future callers vulnerable to omission.

### C — Static Legacy-only API types

Legacy handles and mounts could be made statically Legacy-only. The current dynamic `ChannelId` call graph still needs runtime source validation at webhook and queue boundaries, and the resulting public/API churn would expand compatibility scope. It is therefore unsuitable for Stage C.

## 6. Chosen behavior / 选定行为

### Lifecycle

`ChannelRegistry::start_channel`, `stop_channel`, `restart_channel`, `start_all`, and `stop_all` remain the compatibility entry points. They inspect the canonical registration once at the boundary. Legacy entries use the existing Legacy operation marker and release the source gate before channel I/O. Bound entries use the existing source-owned binding operations and await their returned lifecycle result outside the source gate. No raw Bound `Arc` is returned to a Legacy caller.

The Bound restart compatibility path is a source-owned stop/start sequence using the existing entry-local driver, residual, forwarder, provider, and source checks. It does not create a second restart coordinator. Health monitor restart therefore cannot invoke Legacy `channel.stop()`/`channel.start()` directly for a Bound entry.

### Effects and queue

Every Registry effect helper performs its Bound/Legacy decision before acquiring a channel read guard or beginning adapter work. A denial cannot become `Ok(None)`, `false`, an empty result, or a fallback Legacy permission at the caller.

`send_reported` keeps its existing early refusal before flush. `send_attempt` remains the replay transport choke point. Media custody and `enqueue_fresh` remain after the cheap policy and registration checks. The raw `DeliveryStore::enqueue` seam is changed from a callable dynamic admission surface into a private store operation reached only by a small crate-private queue helper that first asks the existing `ChannelRegistry` to validate the current Legacy registration. Existing direct fixture calls are updated to use that validated helper. There is no unvalidated `ChannelId`-only queue write available to production callers.

### Webhook

The existing mount table remains the only mount collection. A mount carries enough existing source publication identity for the Registry-owned dispatch guard to confirm that its channel id still denotes the same active Legacy registration. The endpoint keeps the existing security order: table lookup, HMAC verification, then source/registration validation, status validation, handler dispatch, and inbound forwarding. HMAC verification is pure parsing/authentication; source validation must complete before any handler parsing, transport effect, or inbound send. A missing, replaced, retired, Bound, or stale mount therefore fails closed and produces no handler or inbound effect.

Signature verification remains before detailed channel-state responses for mounted Legacy traffic. The source guard must not hold `lifecycle_sync` across HMAC, handler parsing, or inbound send. No Bound production ingress is introduced.

### Cold reads

`list`, status summaries, health snapshots, directory reads, source snapshots, and capture-like reads use immutable canonical metadata and existing provider/intent observations. They do not call `start`, factory creation, client construction, directory adapter I/O, forwarder creation, provider activation, or queue admission. Reads do not repair ownership bindings or turn a projection into permission. Existing `list`-style metadata projections may include Bound rows; `get` and all Legacy handle-returning methods remain Legacy-only. Visibility of a row is not execution authority.

## 7. Contract tests / Contract tests

Tests are written before implementation changes and must first reach behavioral RED after parser/import/compile setup issues are corrected.

- `bound_compatibility_effect_paths_are_denied`: exercise Legacy effect helpers, queue boundary, and a cold/ready Bound entry; observe zero loopback request, queue row, ticket, transport call, or fallback permission.
- `bound_held_webhook_handler_denies_dispatch`: use a real held/mounted Webhook handler and source retirement/replacement state; prove the endpoint rejects before direct handler dispatch or unified Legacy inbound delivery.
- `bound_cold_lists_never_call_transport`: repeat cold/discovered/disabled list, status, health, directory, snapshot, and capture-like reads; assert zero client build, request, forwarder, queue, and provider activation events.
- `bound_compat_lifecycle_uses_source`: call compatibility start/stop/restart through Registry; prove the existing source-owned driver/provider/entry path is used, the source gate is not held across I/O, and failed cleanup does not revive Bound.
- `legacy_positive_controls_unchanged`: run equivalent Legacy Webhook start/stop/send/mount operations and assert existing loopback, request, mount, and cleanup behavior.

Tests use real loopback, `ResourceProbe`, held lifecycle barriers, actual Registry state, queue persistence, and source publication boundaries. They do not mutate `active_work`, fabricate receipts, or use a fake permission registry.

## 8. Constraints and verification / 约束与验证

Before every Cargo command, run `python qa/browser_live/check_memory.py` and require exit `0` with `ALLOW`. Record the actual exit code, passed/failed/ignored counts, and any non-fatal linker warning.

Required verification after implementation:

```text
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo check -p alephcore --tests
cargo fmt --all --check
git diff --check
```

If Webhook or capability boundaries change, also run the matching `gateway::interfaces::webhook::` and `capability::` suites. Perform one independent read-only review; if unavailable, record the exact service error and perform the same checklist manually without calling it an independent pass.

## 9. Evidence and remaining boundary / 证据与剩余边界

Only after all five tests are GREEN and review is complete, update the Stage C section in `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`, create a bilingual Stage C run record under `docs/superpowers/runs/`, and correct only the relevant `FEATURE_LOCATOR.md`/Gateway reference entries. Preserve R1 NO-GO/deferred and all Stage D, Feishu/R2, production mount, Zahir, public ingress/send, EffectClaim, Safe Replay, durable recovery, scheduler, and remaining-provider exclusions.

The first contract intentionally left open is the complete all-channel resource matrix and production runtime mount. Stage C cannot claim process isolation, crash recovery, external exactly-once effects, all-provider lifecycle closure, or public Bound invocation.

## 10. Builder decisions left open / 留给 builder 的实现决定

The builder may choose the smallest concrete wiring for the Registry-owned webhook validation callback and the internal visibility boundary for raw queue import helpers, provided the choices preserve the authority and side-effect ordering above. These choices must not add a new authority, generalized envelope, public Bound route, or source-gate-held I/O.

用户批准本设计后，下一步才是调用 writing-plans skill 编写实施计划；在批准前不执行生产代码修改、测试 scaffold、依赖安装或 implementation skill。

---

**Approval / 批准:** 待确认 / Pending
