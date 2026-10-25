# Aleph Phase 2B B8.2 Closeout — Feishu lifecycle implementation and re-gate

> 当前结论：B8.2-A/B/C/D 均为 **NO-GO / deferred**。本记录按实际工作树与当前源码更新；它不把 Feishu provider slice 描述成 Zahir Capability，也不允许 production Feishu → Zahir mount。
>
> 记录时间：2026-10-25

## 1. 实际 baseline

- `pwd`: `/Volumes/TBU4/Workspace/Aleph`
- Git root: `/Volumes/TBU4/Workspace/Aleph`
- `HEAD`: `54df54b8b6a42ba3a03c7edb4d36d31b5d33df34` (`fix: restore TUI test imports after terminal cleanup`)
- Branch: `main` (`main...origin/main`)
- Worktree at the start of this documentation pass was clean with respect to source, tests, and dependencies. This pass modifies exactly three documentation files—`docs/reference/ARCHITECTURE.md`, `docs/reference/FEATURE_LOCATOR.md`, and this closeout—and leaves those documentation edits uncommitted.
- No source changes were made by this documentation pass.

## 2. 已实现的 current source facts

### Gateway authority and intent

`src/gateway/channel_registry.rs:187-220` keeps `channels` as the instance authority, `desired_state` as the intent authority, and `lifecycle_sync` as the shared serialization boundary. `register_with_boot_intent` acquires that boundary before policy preflight, Feishu teardown, unmount, replacement, and intent publication (`src/gateway/channel_registry.rs:385-433`). `create_channel` observes replacement policy before teardown and writes `Enabled`/`Stopped` from `ChannelConfig.enabled` only after successful replacement (`src/gateway/channel_registry.rs:295-350`). `start_all` selects only `Enabled` intent (`src/gateway/channel_registry.rs:673-688`); explicit `start_channel` remains available for a disabled registration. `stop_channel_inner` records `Stopped` before stopping (`src/gateway/channel_registry.rs:639-665`). `restart_channel_inner` writes `Running` only after stop, start, and mount refresh succeed; failed restart after a successful stop clears intent to `Stopped` (`src/gateway/channel_registry.rs:1460-1504`).

Boot wiring passes Feishu's explicit enabled decision into `register_for_boot` (`src/bin/aleph-server/commands/start/builder/subsystems.rs`). Therefore disabled boot does not enter `start_all`, and no Feishu token acquisition, API client runtime, webhook listener, WS runtime, emitter publication, or webhook mount is created by the disabled boot path.

### Feishu lifecycle and fences

`FeishuChannel` constructs `ProviderLifecycle` in `Discovered` and immediately admits the instance into `Cold` (`src/gateway/interfaces/feishu/mod.rs:88-94`; `src/capability/provider.rs:51-62,112-120`). `Cold` means an admitted declaration with no runtime effects and no executable lease; it must not be described as `Ready` or as a successful start. `start` then performs the only production transition into `Starting` before runtime work (`src/gateway/interfaces/feishu/mod.rs:338-355`), and commits `Ready` together with derived publication through `api_handle::publish` (`src/gateway/interfaces/feishu/mod.rs:334-377`; `src/gateway/interfaces/feishu/api_handle.rs:42-70`). A start failure revokes publication, cleans up, records `Failed`, and returns the error (`src/gateway/interfaces/feishu/mod.rs:367-381`).

`stop` revokes first and then cleans up refresh, WS, webhook serve, and API calls (`src/gateway/interfaces/feishu/mod.rs:109-183,384-405`). Webhook shutdown is bounded by `WEBHOOK_SHUTDOWN_TIMEOUT`; serve/join/refresh/WS failures are retained as `FeishuShutdownResidual`. `run_webhook_server` awaits Axum graceful shutdown (`src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:29-44`), but `FeishuChannel` does not own every accepted handler task. A timeout therefore records `HandlersUnjoined`; it is not a complete per-handler ownership/join proof, and the channel reports failure rather than false `Stopped`.

`FeishuApi` holds the shared calls read guard across the entire HTTP call, including token acquisition, and `drain` waits for the write side (`src/gateway/interfaces/feishu/api.rs:61-115`). Webhook admission uses the same guard and rejects stale requests with `503` (`src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:48-120`). `api_handle` is a derived `Weak<FeishuApi>` cache, not authority; `publish` validates API scope, provider instance, generation, and incumbent occupancy, while `withdraw` and stale `get` remove only the exact provider instance/generation (`src/gateway/interfaces/feishu/api_handle.rs:1-104`). An old generation cannot publish over or withdraw a replacement.

## 3. Source-backed call graph and authority mapping

```text
boot builder (explicit Feishu enabled)
  -> ChannelRegistry::register_for_boot
  -> ChannelRegistry::start_all
  -> start_channel_inner
  -> FeishuChannel::start
  -> start_runtime
     -> TokenManager / FeishuApi / webhook server OR FeishuRuntime WS
  -> api_handle::publish
  -> ProviderHandle::with_ready_commit(Ready + derived cache)

outbound channel action
  -> FeishuChannel::admit_call
  -> ProviderHandle::acquire_call(current generation)
  -> FeishuApi::admit (calls read guard + provider admission)
  -> Feishu HTTP request

inbound webhook / WS
  -> handler or WS forwarder
  -> FeishuApi::admit_inbound / provider admission
  -> ChannelState sender

stop / replacement
  -> ChannelRegistry lifecycle boundary
  -> FeishuChannel::revoke
  -> api_handle::withdraw(exact instance + generation)
  -> cleanup producers and webhook serve
  -> FeishuApi::drain
  -> mount removal only after successful stop path
```

| Authority | Current source-backed responsibility |
|---|---|
| `ChannelRegistry` | Channel instances, enabled/desired intent, lifecycle serialization, start/stop/restart/replacement, webhook mount table. |
| `ProviderHandle` | Feishu provider state, generation, and call admission fence; it is not a registry or enabled-state authority. |
| `FeishuApi` | Runtime client and one shared in-flight admission/drain guard; it does not decide channel intent or capability visibility. |
| `api_handle` | Derived Feishu client/config publication cache; never a durable source or activation path. |
| `OwnershipTree` / `ZahirFacade` | Existing Tool capability owner/lease/projection path only; no Feishu binding or adapter is mounted. |
| `SessionEventStore` | Durable session event authority; no Feishu provider lifecycle producer is present. |
| broadcast/live notifications | Live notification only; no durable replay or recovery authority. |

The Tool projection path has two bounded live-notification layers. `OwnershipTree` sends committed mutations through its own `tokio::sync::broadcast` with capacity **8** (`src/capability/ownership.rs:246-285`); a receiver that falls behind gets `Lagged`, which is notification loss rather than durable history. `ProjectionHost` handles both an ownership event and `OwnershipTree` lag by invalidating consumers and rebuilding from a fresh applied snapshot (`src/capability/projection_host.rs:1401-1417`). Separately, each consumer's pending delivery queue defaults to **64** events (`src/capability/projection_host.rs:55-65`); queue overflow atomically clears stale pending items and enqueues `Invalidated` plus one replacement `Snapshot`, covered by `default_overflow_replaces_backlog_with_invalidated_pair` (`src/capability/projection_host.rs:1936-1992`). Registry broadcast remains a 256-slot live notification path. None of these streams is durable recovery, and owner generation, registry cursor, and `SessionEvent.seq` remain independent domains.

## 4. Formal B8.2 re-gate

| Gate | Formal meaning | Verdict | Source-backed blocker |
|---|---|---|---|
| A | Descriptor and an existing `CapabilityKind` uniquely express the externally composable capability, with one descriptor authority and pure projection. | **NO-GO / deferred** | No Feishu descriptor adapter or selected existing kind is wired. `ZahirFacade` production wiring is Tool-only; `CapabilityKind` and descriptor sources do not describe Feishu routes/admission. |
| B | Lease, provider generation, owner generation, visibility/reference, and replacement semantics bind to one authority and stale handles fail closed. | **NO-GO / deferred** | Feishu `ProviderHandle` generation fences channel/API calls, but it is not a Zahir owner generation: no Feishu `BackendLease` is issued or validated through `ZahirFacade`, no Feishu descriptor is bound to `OwnershipTree`, and no production route connects provider-instance replacement to Zahir owner invalidation. The existing `BackendLease`/`OwnershipTree` contract is therefore Tool-only evidence, not Feishu B-gate evidence. In addition, `src/capability/ownership.rs:491-499` uses `inner.nonce = inner.nonce.saturating_add(1)` in `OwnershipTree::bump`; after `u64::MAX` it can repeat a generation, weakening the stale-lease fail-closed proof. This residual risk is unresolved. |
| C | Callable route passes resolve → lease validation → actual provider admission, and external effects have an explicit boundary. | **NO-GO / deferred** | There is no Feishu resolve/callable route. Webhook is a Gateway-owned external effect; no Feishu `EffectClaim` producer, Safe Replay, scheduler, or external exactly-once contract exists. |
| D | Live observation has subscribe-before-snapshot plus invalidation/resync, and durable recovery has one explicit authority. | **NO-GO / deferred** | Feishu has no Capability subscribe/project or durable provider lifecycle producer. Tool Zahir broadcast is scoped to Tool projection; `api_handle` and provider notifications are live-only. `SessionEventStore` remains session durability, not Feishu lifecycle recovery. |

Because A is the first unclosed gate, the phase remains **NO-GO / deferred**. Tool `ProjectionHost`/`ZahirFacade` production mounting is explicitly Tool-only evidence and cannot be used as Feishu evidence.

## 5. Failure matrix

| outcome | publication | resource | observer |
|---|---|---|---|
| **Disabled boot** — registry records `Stopped`; `start_all` returns no Feishu start; explicit start remains possible. | No `api_handle` publication and no webhook mount. | No Feishu token/API runtime, webhook listener, WS runtime, refresh task, or emitter runtime is created by boot. | No Feishu Capability observer or lifecycle recovery source exists. |
| **Normal ready** — Feishu reaches `Connected`/`Running` only after successful runtime start and Ready commit. | Derived cache publication is fenced by API scope, provider instance, and generation; it is not a capability publication. | Token manager, API client, webhook server or WS runtime, refresh task, and channel forwarder are live according to config. | Channel health/list surfaces can observe Gateway state; no Feishu Zahir subscription exists. |
| **Refresh/start failure** — startup returns the real error and provider settles `Failed`; no false Ready. | `revoke` removes any exact publication; failed start does not publish a new generation. | Cleanup aborts/joins owned refresh/WS work, drains API calls, and retains residuals if joins fail. | Health remains disconnected/failed; no durable Feishu recovery event is emitted. |
| **Webhook or WS failure** — bind/serve/join failure is returned and retained as residual. | Publication is revoked before cleanup; replacement is refused when stop leaves residual evidence. | WS join, webhook serve/join, refresh join, or `HandlersUnjoined` residual remains attached to the channel. | Gateway health can expose failure; no capability observer/resync exists. |
| **Stop timeout / handler residual** — `stop` returns error and transitions `Stopping` to `Failed`; it does not claim complete handler drain. | Exact cache entry is withdrawn; a failed replacement path retains the incumbent registration and may retain its mount until explicit recovery. | `FeishuApi::drain` proves admitted-call/enqueue quiescence only; accepted handler task ownership/join remains unproven. | No durable provider recovery; residual is local failure evidence. |
| **Replacement policy failure** — policy observation fails before Feishu teardown; incumbent remains registered and live. | Incumbent publication/mount remain untouched by the preflight refusal. | Incumbent runtime is untouched; DeliveryStore fail-closed witness remains active. | No Feishu capability change is emitted. |
| **Stale generation / late completion** — `ProviderHandle::acquire_call` rejects stale state; old calls fail closed. | `publish`/`withdraw`/stale `get` are scoped to exact instance and generation, so old work cannot overwrite or remove a replacement. | Old API guards drain independently; replacement owns its own generation. | No Feishu live stream; stale effects do not become observer history. |
| **Live observer lag** — no Feishu observer contract is installed. | Tool broadcast may lag and invalidate/resync only the Tool projection. | Feishu cache/provider notifications are not a replay buffer. | Feishu lag/resync is deferred; broadcast is notification-only. |
| **Durable recovery** — session durability remains available through `SessionEventStore`. | No Feishu provider publication is reconstructed from session events. | No second durable store and no Feishu lifecycle producer exist. | Feishu provider recovery is deferred; `api_handle` and broadcast are not recovery sources. |

## 6. Verification evidence

Final validation evidence is recorded in [`task-5-report.md`](/Volumes/TBU4/Workspace/Aleph/.superpowers/sdd/2026-10-24-aleph-phase2b-b8-2-feishu-implementation/task-5-report.md). The report records passing format, focused Feishu tests (9/9), `cargo test -p alephcore --lib feishu` (107/107), the full `alephcore` library run (21,521 passed, 20 ignored), workspace compile checks, and Clippy with three unrelated warnings. `qa/channels/run.sh all` passed phases 1 and 2; phase 3 remains **BLOCKED/UNRUN** because `qa/channels/drive_approval.py:38` raised the exact error `ModuleNotFoundError: No module named 'websockets'`.

The focused tests cover disabled boot, explicit enable, lifecycle state, API publication fences, stale publication cleanup, inbound admission/drain, WS join, webhook residuals, replacement ordering/preflight, create/restart intent, and lifecycle serialization. These counts are historical evidence from the implementation validation pass; this documentation pass did not rerun cargo or QA. The source tree was clean before documentation edits began, and this pass leaves exactly three documentation files uncommitted. No Rust source, tests, dependencies, or Git history were changed by this pass.

## 7. North Star and residuals

The current code improves lifecycle isolation and failure honesty: disabled boot avoids Feishu runtime effects; registry intent and instance replacement are serialized; provider/API generations fence stale calls and publication; stop residuals remain observable; and the Core has no Feishu-specific capability mount. The remaining work is contract closure, not facade glue: select and prove an existing capability kind, bind Feishu lease/owner/provider generations without duplicate authorities, define callable/effect semantics, and add live invalidation plus durable provider recovery evidence.

## 8. Not done / negative scope

- No Feishu descriptor, `CapabilityKind` selection, `BackendLease` route, Zahir resolve/callable adapter, Feishu subscribe/project surface, or production mount.
- No EffectClaim producer, Safe Replay execution, external-effect exactly-once, universal scheduler, second registry, second enabled/owner/generation map, or second durable store.
- No claim of hot unload, code unmapping, complete per-handler ownership/join, or durable Feishu provider recovery.
- This pass changed three documentation files only; it did not change Rust source, tests, dependencies, or Git history. Those documentation edits remain uncommitted.
- Full channel QA phase 3 remains unvalidated until the existing `websockets` dependency is available in the QA environment.
