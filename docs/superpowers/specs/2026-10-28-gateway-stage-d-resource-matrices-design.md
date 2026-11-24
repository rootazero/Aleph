# Stage D — Source-Owned Runtime Resource Matrices Design

- **Status / 状态**: APPROVED — human-approved 2026-10-28
- **Date / 日期**: 2026-10-28
- **Branch / 分支**: `stage-d-resources`
- **Base / 基线**: Stage C HEAD `47625530cbc26da41a791caa09b8c7cfd0b485d3`
- **Parent plan / 父计划**: `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` §§5–7

## 1. Intent and why / 目的

Stage D must prove that the existing source-owned Gateway lifecycle really allocates and releases provider resources, rather than merely changing visible state. It adds executable evidence for one real channel and one independently implementable channel shape.

Stage D 只验证并必要时修复现有通用生命周期语义：cold 操作不得分配 provider 资源，active/stop/replacement 必须有真实资源 receipts，失败不能伪造零残留。它不是新的生产 channel、生产注册入口或资源控制平面。

## 2. Goals and non-goals / 范围

### Goals

1. Run the same cold/active matrix against:
   - the real Generic `WebhookChannel`;
   - a test-only `PollingSocketChannel` with real loopback TCP, worker `JoinHandle`, timer, provider cache and temporary SQLite database.
2. Prove cold registration, list, discovery snapshot, projection, capture, resolution, rejected admission, stale-proof reads and unregister allocate no provider resources.
3. Prove start, real send, stop, replacement, stale generation/proof rejection and repeated cleanup have observable effects and receipts.
4. Prove partial activation and failed cleanup expose real residual resources; successful cleanup returns measured active resources to zero.
5. Add positive controls so zero cannot result from inactive hooks, skipped start or a missing observer.
6. Keep Stage A–C suites and public compatibility behavior green.

### Non-goals / 明确不做

- No production Bound registration opt-in, second descriptor/source/cleanup map or new CapabilityKind.
- No production `ResourceTracker`, generic polling provider, Feishu/R2, terminal, memory graph or sandbox implementation.
- No new public registry visibility, public `ResourceProbe`, fake cleanup receipt or state-derived zero.
- No new dependency, deferred cleanup drain, detached cleanup task or production mount.
- No Stage E execution/readiness work, universal lifecycle/mesh/persistence model or R1 claim.

## 3. Current evidence and constraints / 现状与约束

- `ChannelRegistry` remains the sole Gateway association authority. `EntryRuntime` holds entry-local lifecycle bookkeeping; `ProviderHandle` owns provider instance/state/generation; `OwnershipTree` owns binding/lifetime/generation/revoke/dispose.
- Existing `register_bound_channel`, `capture_bound_channel`, `snapshot_bound_channels`, `resolve_bound_channel`, `start_bound_channel[_compat]`, `stop_bound_channel[_compat]` and `unregister_bound_channel` are sufficient. They remain `pub(crate)` and need no signature changes.
- Real Generic Webhook observation uses the existing `ResourceProbe` events at actual client/request boundaries: `client_build_attempt`, `client_built`, `client_drop`, `request_attempt` and `request_ended`, plus loopback HTTP delivery. Private Webhook context is not a polling/socket/DB receipt.
- Existing `LoopbackPeer` provides real raw-TCP peer behavior; `TempDir` provides isolated SQLite paths. Existing tests prove failed Bound stop retains the incumbent and residual marker. A retry currently remains `Busy`; Stage D does not change that recovery semantic.
- The source plan requires observation at real connect/open/close boundaries. Merely moving a mock from a constructor into `start` is not resource-lifecycle evidence.
- Production behavior stays unchanged unless a behavioral RED proves a generic defect. Existing `cfg(test)` observation may be extended where needed, but it must record a real operation and must not change lifecycle semantics. A defect fix must be minimal, independently reviewed and must not widen Stage D scope.

## 4. Approaches and decision / 方案比较与决定

### A. Dedicated matrix module plus shared test-only fixture — selected

Add `src/gateway/channel_registry/resource_matrix.rs` as a `#[cfg(test)]` sibling module. Put the reusable fixture and receipt guards in the existing `channel_registry/test_support.rs`.

**Benefits:** one set of matrix assertions and observation semantics for both shapes; no further growth of the already large registry test module; no production API change.

**Costs:** a new test module and a modest amount of test-only fixture code. Some test setup must be local because existing inline-test helpers are private; this is acceptable and does not duplicate runtime authority.

### B. Append all matrices to `channel_registry.rs`

This avoids one file but mixes several hundred lines of resource fixtures and matrices into the existing large test module. It makes receipt auditing and later Stage E review harder, so it is rejected.

### C. Add a production resource-observation framework

A generic production `ResourceTracker` would create another resource fact source and exceed the approved evidence stage. It is rejected.

## 5. Test architecture / 测试结构

### 5.1 Files

- `src/gateway/channel_registry/resource_matrix.rs`
  - matrix tests, shape setup, common assertions and positive controls;
  - compiled only under `cfg(test)`.
- `src/gateway/channel_registry/test_support.rs`
  - `PollingSocketChannel`, its configuration and worker;
  - test-only resource receipt counters/RAII guards;
  - loopback peer reuse and failure controls.
- `src/gateway/channel_registry.rs`
  - one `#[cfg(test)] mod resource_matrix;` declaration;
  - production implementation unchanged unless a proven generic defect requires a fix.
- `src/gateway/interfaces/webhook/mod.rs`
  - existing `ResourceProbe` observation is reused;
  - if required for the failed-stop owner-drop case, a `cfg(test)` `Drop` observation records the real client destruction. It adds no production behavior or API.

No existing production file becomes dependent on the test fixture.

### 5.2 Common shape harness

A test-only `Shape` enum selects `RealWebhook` or `PollingSocket`. Setup returns the registered channel ID plus a shape-specific receipt handle. The matrix body and assertions are shared; setup may use a small `match` because the concrete channel constructors differ.

The harness must not define a second runtime registry, ownership map or lifecycle state machine. All registration, start, stop, resolution and cleanup operations go through the existing `ChannelRegistry` and `OwnershipTree`.

### 5.3 Receipt model

Receipts are test-only observations, not a production authority:

- **RealWebhook:** existing `ResourceProbe` counters at actual client/request boundaries and actual loopback HTTP response/body evidence. `client_build_attempt`/`client_built` must both be nonzero in the active control; `client_drop` is observed only after real stop or owner drop. Assertions are limited to these observed operations; no claim is made about sockets, tasks or databases that Generic Webhook does not own.
- **PollingSocket:** counters/guards updated at actual resource construction and destruction:
  - TCP connect attempts and stream drop;
  - worker spawn, normal exit and `JoinHandle` join;
  - timer creation/drop and at least one observed tick during the active positive control;
  - fixture-provider-owned cache creation/drop;
  - SQLite open attempts, successful open, actual `Connection::close` result and connection drop.

The cache belongs to the test channel as a provider implementation resource; it is not attributed to `ProviderHandle`, which has no production cache. It proves that a second provider shape's real cache is absent while cold and released by the generic lifecycle, not that a nonexistent Core cache was cleaned.

`active == 0` is calculated from actual construction/destruction receipts, never from `ChannelStatus`, `ProviderState`, desired state or a successful return value alone. A failed start still records its real connect/open attempts.

## 6. PollingSocketChannel contract / 测试 fixture 契约

### 6.1 Cold construction

The channel constructor stores only configuration, failure controls and receipt handles. It does not connect, open SQLite, create a provider cache/timer or spawn a worker. Registry discovery may read identity/config but receives no private hook context.

### 6.2 Start and worker ownership

`start` performs real fallible operations:

1. increment the connect-attempt counter and connect a `TcpStream` to a loopback peer;
2. open a SQLite connection at the test temporary path and execute a minimal schema statement;
3. create the provider cache and interval timer;
4. spawn one worker which owns the stream, connection, cache, timer and cancellation receiver;
5. store the worker's `JoinHandle` in the channel so stop can join it.

The worker performs periodic observable ticks until cancellation. On cancellation it closes SQLite, drops the cache/timer/stream and records worker exit before returning. No cleanup is detached or left without a tracked join.

### 6.3 Send and stop

`send` is an owned `SendOperation`, consistent with Stage C, and succeeds only after the active fixture records the attempted payload. It does not perform external network sends.

`stop` cancels and joins the worker, then reports the measured receipts. It has no retry/recovery failure mode in this fixture. Failed-cleanup evidence uses the existing real-Webhook-backed `FailingStopWebhook` so Stage D does not invent a new recovery protocol or leave a polling worker without a registry-owned cleanup path.

## 7. Matrix behavior / 行为矩阵

### 7.1 Cold matrix — both shapes

For each shape and each `ChannelInitialState::{Discovered, Cold, Disabled}`, before and after every operation assert zero active provider resources and zero worker/timer/connection/cache/database creation where applicable:

1. register cold;
2. list/status/health and discovery `snapshot_bound_channels`;
3. project a description and capture a bound channel;
4. attempt resolution with the correct ownership binding, provider generation and scope; assert fail-closed provider admission because a cold provider is not `Ready`, and assert that the attempt did not start the channel;
5. reject missing/wrong ownership or visibility admission independently of provider-state admission;
6. capture proof, unregister and reject subsequent old-proof capture validation/resolution.

A `ProviderHandle`, descriptor or registration entry is not itself counted as a socket/thread/DB/cache resource.

### 7.2 Active lifecycle matrix — both shapes

For each shape:

1. cold-register and assert the cold zero baseline;
2. start and assert positive resource controls;
3. perform a real send and assert the shape's actual delivery/payload receipt;
4. stop and assert worker/timer/connection/cache/database or public Hook cleanup receipts;
5. assert measured active resources are zero only after the real cleanup/join completes;
6. replace the running same-ID incarnation through the existing `replace_bound_channel`, which must retire the old resources before publishing the fresh instance, and prove old capture/provider generation cannot resolve or call the replacement;
7. repeat stop/unregister read paths and verify no duplicate cleanup or false receipt.

The real Webhook positive control uses its actual public Hook and loopback HTTP request. The polling positive control requires a live TCP connection, opened DB, cache/timer, worker and observed tick before stop.

### 7.3 Failure matrix

- **RealWebhook start failure:** use an invalid real `WebhookChannelConfig` so `validate()` fails before `client_build_attempt`; assert error propagation and zero client/request resources. Generic Webhook does not bind a socket in `start`, so occupying a port is not a valid failure injection.
- **Polling connect failure:** use a closed loopback port; connect attempts increase, no successful resource is reported.
- **Polling partial activation:** use a valid loopback listener but an invalid SQLite open path; prove the already-connected stream is dropped and no worker/cache/timer/DB remains.
- **Real Webhook failed stop:** start the existing real-Webhook-backed `FailingStopWebhook`; its injected stop error leaves the real reqwest client live and the incumbent registered. Assert measured residual is nonzero and that retry/unregister remain `Busy` under current semantics. Dropping the owning registry/channel then performs real client destruction, observed by `client_drop`; no retry-success or deferred-cleanup claim is made.

Every failure test performs final real cleanup through a supported owner path so the test process leaves no tracked worker or server task behind.

## 8. Test cases and RED discipline / 测试与 RED

Planned tests in `gateway::channel_registry::resource_matrix::`:

1. `cold_operations_allocate_no_resources_for_either_shape`
2. `active_send_stop_and_old_proof_contracts_hold_for_either_shape`
3. `replacement_cleans_old_resources_and_rejects_old_provider_proof`
4. `start_failures_do_not_fabricate_resources_or_cleanup`
5. `failed_stop_preserves_real_residual_until_owner_drop`
6. `active_resource_observers_have_positive_controls`

The new polling tests must initially fail against a deliberately incomplete test-only fixture or absent implementation for a valid behavioral reason (for example, missing start/stop/resource behavior). Parser, import and compile failure is not contract RED. Existing real-Webhook/generic behavior may pass immediately; that is a valid positive control, not a reason to manufacture a production RED.

If a matrix exposes a production defect, add the smallest focused behavioral RED first and stop for architecture direction if the only fix requires public lifecycle changes, a second authority or deferred cleanup.

## 9. Verification and independent review / 验证

Every Cargo command is individually preceded by:

```bash
python qa/browser_live/check_memory.py
```

Require exit 0 / `ALLOW`. Record command, exit code and passed/failed/ignored counts. Zero-test filters are failures.

Run the new resource-matrix filter, both original mid-send tests three serial times each, then the complete Stage C set:

```bash
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::
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

A fresh read-only reviewer inspects the complete Stage D diff and relevant Stage A–C contracts, with special attention to false-zero receipts, cold allocations, failed cleanup, stale proof, source identity, lock scope, detached tasks and authority duplication. If unavailable, record the exact error and perform the same manual checklist without claiming independent approval.

No Critical/Important finding may remain before Stage D is recorded as closed.

## 10. Documentation and residual trade-offs / 文档与残余限制

After green verification and review, update the bilingual run record, source plan §5 and `FEATURE_LOCATOR.md` with the actual matrix counts, real resources observed, reviewer identity/verdict and remaining boundaries.

This evidence does not make the test-only polling channel a production provider, prove resources Generic Webhook does not own, implement terminal/memory-graph/sandbox lifecycle isolation, or authorize Stage E. Failed Bound stop remains non-retryable/`Busy` under the current lifecycle contract; changing that recovery semantic is a separate architecture decision. These limitations must be stated explicitly rather than hidden behind a generic “resource-safe” claim.

## 11. Builder-check resolutions / 实施者检查结论

A fresh builder-check reviewer inspected the draft and actual code. Its five material questions are resolved as follows; these are the designer's calls, not claims that the reviewer approved the design:

1. **Failed-stop retry:** current Bound semantics leave the operation marker and `Stopping` state, so retry/unregister remains `Busy`. Stage D will test that fail-closed residual contract and final owner-drop cleanup, not introduce retry recovery.
2. **Cold resolve:** `resolve_bound_channel` includes provider call admission, which requires `Ready`; cold resolution must fail closed without allocating/starting resources. Successful resolution is tested only in the active matrix.
3. **Real Webhook start failure:** Generic Webhook does not bind a socket in `start`; invalid configuration is the real pre-allocation failure. Partial activation after resource acquisition belongs to the polling fixture.
4. **Webhook receipts:** the design now uses the actual existing `ResourceProbe` event vocabulary. No fictitious `GenericHook` API or new receipt framework is assumed.
5. **Provider cache:** this is a real cache owned by the test provider implementation, not a fictitious cache inside Core's `ProviderHandle`. Its evidence is explicitly limited to the second provider shape.

The cold matrix also covers all three existing initial states (`Discovered`, `Cold`, `Disabled`) so no initial-state variant can accidentally allocate resources.
