# Gateway Bound Compatibility Closure — Stage C Plan

日期 / Date: 2026-10-27

## 目标 / Goal

在已完成 Stage B 的 source-owned Bound retirement、replacement、unregister 之上，闭合 Gateway 现有 Legacy compatibility surfaces，证明 Bound channel 不会通过旧的 raw handle、Option/bool refusal、webhook wrapper、health/restart、directory/list、delivery queue 或 inbound route 误获得执行权限。

This stage closes compatibility boundaries around the existing Gateway registry. It does not add a public Bound API or production mount.

## 起点 / Starting point

- Base commit: `95f388c41 docs(gateway): record bound retirement evidence`.
- Stage B code commits: `a6d122bf4`, `df513848c`, `255ebc8a6`.
- Existing authority split:
  - `ChannelRegistry` owns the canonical `channels` map/entry, publication revision, source identity, entry-local tickets, lifecycle markers, cleanup drivers and residuals.
  - `ProviderHandle` owns provider instance/state/generation.
  - `OwnershipTree` owns owner/lifetime/generation/revoke/dispose.
- Stage B evidence is recorded in `docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`.
- R1 remains NO-GO/deferred.

## 硬边界 / Hard boundaries

- Scope is Stage C compatibility closure only.
- Do not enter Stage D resource matrices or add a second polling/socket shape.
- Do not enter Feishu/R2, production Bound opt-in/mount, Zahir route, public Bound send/ingress, EffectClaim producer, Safe Replay, durable recovery, universal scheduler, new `CapabilityKind`, generalized envelope, second registry, owner map, descriptor map, or `src/harness/**`.
- Do not silently migrate callers. Inspect existing callers first; if a boundary cannot be closed centrally, report a scoped architecture amendment.
- Keep production `register_bound_channel` callers absent outside tests.
- Every real I/O operation remains outside `lifecycle_sync`; compatibility denial must happen before ticket creation, transport access, queue write, or effect.
- Test observations must use real loopback/effect boundaries and actual registry state. Do not use fake receipts or manual runtime-state mutation as authority.
- Before every cargo command run `python qa/browser_live/check_memory.py`; record the result and all test counts.

## 必须先读取 / Required reads

- `AGENTS.md`
- `docs/reference/FEATURE_LOCATOR.md`
- `docs/reference/GATEWAY.md`
- `docs/reference/ARCHITECTURE.md`
- `docs/reference/DEVELOPMENT.md`
- `docs/reference/REDLINES.md`
- `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` Stage C
- `docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md`
- `src/gateway/channel_registry.rs`
- `src/gateway/channel_registry/binding.rs`
- `src/gateway/channel_registry/test_support.rs`
- `src/gateway/handlers/channel.rs`
- `src/gateway/handlers/discord_panel.rs`
- boot `subsystems.rs`
- health monitor
- delivery queue
- inbound router
- builtin channel tools
- webhook receiver
- pure slot `Channel` contract

## 执行顺序 / Execution order

### 1. Refresh the compatibility census

Search actual callers and record a table of every Bound-sensitive surface:

- raw `get`/capture and map iteration;
- list/status/health/directory projections;
- start/stop/restart/start-all;
- public/private telecom and SMS paths;
- normal send/retry/outbox;
- edit/react/typing/poll/group-icon helpers;
- webhook verify/handler wrappers and held handlers;
- inbound forwarder and plugin routes;
- health monitor and delivery queue;
- production `register_bound_channel` callers.

For each surface record: caller, current authority, whether Bound is denied, whether denial occurs before resource/effect creation, and the exact test or evidence needed. Do not change code during the first census.

### 2. Write RED tests first

Add tests in the existing Gateway test modules, using real effect boundaries:

- `bound_compatibility_effect_paths_are_denied`
  - Exercise legacy effect helpers against a cold and/or ready Bound entry.
  - Assert no loopback request, queue write, ticket, or transport call occurs.
  - Assert refusal is not consumed as fallback permission.

- `bound_held_webhook_handler_denies_dispatch`
  - Obtain the real webhook handler/held wrapper path.
  - Assert Bound verify/direct handler dispatch is denied before unified Legacy inbound delivery, including after lifecycle retirement/unmount where applicable.

- `bound_cold_lists_never_call_transport`
  - Repeat list/status/health/directory/snapshot/capture-like reads for Discovered, Cold and Disabled declarations.
  - Assert no client construction, request, forwarder, queue, or provider activation.

- `bound_compat_lifecycle_uses_source`
  - Exercise the compatibility lifecycle entry points and prove they use source-owned start/stop/restart decisions.
  - Assert the shared lifecycle gate is not held across real channel I/O and failed cleanup does not implicitly revive a Bound entry.

- `legacy_positive_controls_unchanged`
  - Run the same relevant operations on a Legacy Webhook channel.
  - Assert existing successful start/stop/send/mount behavior remains intact.

Contract RED must be behavioral. Fix parser/import/compile/test setup errors before counting a test as RED.

### 3. Implement the smallest central closure

Only after contract RED:

- Keep Bound raw `Arc` exposure denied.
- Route Bound lifecycle compatibility calls to the existing source-owned Bound operations where a central route is safe.
- Make legacy effect helpers reject Bound before ticket/resource/queue creation.
- Ensure Option/bool refusals cannot become permission or success in callers.
- Keep cold projections metadata-only and sourced from the canonical entry; they must not start transport or repair owner bindings.
- Ensure health restart honors desired intent, `Busy`, residual and `Disabled`, with no implicit `Ready` after failed cleanup.
- Keep source-owned webhook wrappers fail-closed for verify/direct handle/legacy inbound dispatch. This does not implement production EventSource or Subscription.
- Preserve Legacy positive controls.
- If a caller cannot be safely closed by the central registry without widening authority, stop and record an amendment rather than migrating it silently.

### 4. Verify and review

Run, with memory check before every cargo command:

```bash
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo check -p alephcore --tests
cargo fmt --all --check
git diff --check
```

Also run the relevant `gateway::interfaces::webhook::` and `capability::` suites if changed code crosses those boundaries. Record exact passed/failed/ignored counts and the known linker warning if it appears.

Perform one independent read-only review if the reviewer service is available. Review focus: Bound bypasses, raw handle exposure, Option/bool fallback permission, cold read allocation, source-gate/I/O discipline, legacy regressions, and unintended scope expansion. If unavailable, record the exact failure and complete the same checklist manually; never claim independent review passed.

### 5. Evidence and handoff

If all contracts are GREEN, update:

- `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` Stage C checkboxes/evidence;
- a new bilingual Stage C run record under `docs/superpowers/runs/`;
- `docs/reference/FEATURE_LOCATOR.md` and only relevant Gateway/reference sections.

Preserve R1 NO-GO/deferred and all Stage D/E, Feishu/R2, production mount, Zahir, EffectClaim, Safe Replay, durable recovery, scheduler and remaining-provider exclusions.

Suggested implementation commit: `fix(gateway): close bound runtime compatibility bypasses`.
Suggested evidence commit: `docs(gateway): record bound compatibility evidence`.

## 完成标准 / Definition of done

- All five named compatibility tests are real behavioral GREEN tests.
- Bound legacy/effect paths fail before ticket/resource/queue/effect creation.
- Bound cold reads do not initialize transport or provider runtime resources.
- Bound lifecycle compatibility uses the existing source-owned lifecycle path.
- Legacy positive controls remain GREEN.
- No new Gateway authority, CapabilityKind, public route, production mount, or unrelated subsystem change exists.
- Exact verification, review availability, first unclosed contract, skipped validations, and destructive/external actions not taken are recorded.
