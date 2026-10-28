# Gateway Bound Compatibility Closure — Stage C Run Record

**Date / 日期**: 2026-10-28  
**Scope / 范围**: Stage C compatibility closure only / 仅 Stage C compatibility closure  
**Implementation HEAD / 实现提交**: `66489d94b` (`fix(gateway): deny bound compatibility effects before admission`)  
**Status / 状态**: required Stage C suites GREEN; independent review unavailable; two additional SMS integration timeouts remain open / 必需 Stage C suites 全绿；独立 review 不可用；两个额外 SMS integration 超时仍未闭合

## Bounded slice / 限定切片

This change closes the existing Legacy compatibility surfaces against Bound registrations without adding a public Bound send/ingress route. `ChannelRegistry` remains the sole association authority. Existing entry-local runtime, `ProviderHandle`, and `OwnershipTree` contracts are unchanged; no second registry, owner map, descriptor map, runtime table, CapabilityKind, or generalized envelope was introduced.

本次修改仅关闭 Bound 注册在既有 Legacy compatibility surfaces 上的旁路，不新增 public Bound send/ingress route。`ChannelRegistry` 仍是唯一 association authority；既有 entry-local runtime、`ProviderHandle` 与 `OwnershipTree` 契约未改变，也未新增第二份权威。

The implementation covers Legacy effect/queue admission, retained webhook mounts, cold reads, and source-owned lifecycle compatibility. It does not implement Stage D resource matrices, Feishu/R2, production mount, Zahir routes, public send/ingress, EffectClaim, Safe Replay, durable recovery, or a scheduler.

实现覆盖 Legacy effect/queue admission、保留的 webhook mount、cold reads 和 source-owned lifecycle compatibility；不实现 Stage D resource matrix、Feishu/R2、production mount、Zahir route、public send/ingress、EffectClaim、Safe Replay、durable recovery 或 scheduler。

## Contract tests / 契约测试

The five requested tests were written before the corresponding production closure and are now GREEN:

五个要求的测试先于对应生产行为修改编写，现均为 GREEN：

- `bound_compatibility_effect_paths_are_denied`
- `bound_held_webhook_handler_denies_dispatch`
- `bound_cold_lists_never_call_transport`
- `bound_compat_lifecycle_uses_source`
- `legacy_positive_controls_unchanged`

They observe real loopback/counting channels, webhook dispatch, queue admission, entry/provider/forwarder state, and source publication boundaries. No fake receipt or manually injected active-work permission is used.

测试观察真实 loopback/counting channel、webhook dispatch、queue admission、entry/provider/forwarder 状态与 source publication boundary；未使用 fake receipt 或手工注入 active-work 权限。

Initial Task 1 RED evidence: `cargo test -p alephcore --lib gateway::channel_registry::tests::` completed with 2 passed and 3 behavioral RED failures; parser/setup errors were repaired before recording behavioral RED. The lifecycle and raw queue REDs were subsequently closed in `ae3df80ef` and `66489d94b`.

Task 1 初始 RED 证据：Registry suite 为 2 passed、3 个真实行为 RED；parser/setup 问题先修复，再记录行为 RED。lifecycle 与 raw queue RED 随后分别由 `ae3df80ef`、`66489d94b` 闭合。

## Required command ledger / 必需命令记录

Every Cargo command below was immediately preceded by `python qa/browser_live/check_memory.py`, which exited 0 with `ALLOW`. Commands ran from the repository/package working tree, not by invoking tests from an unauthorized root harness. The only linker diagnostics were non-fatal macOS `ld` warnings; no linker failure occurred.

以下每条 Cargo 命令前均立即运行 `python qa/browser_live/check_memory.py`，均 exit 0 / `ALLOW`。仅出现非致命 macOS `ld` warning，无 linker failure。

| Command / 命令 | Exit | Result / 结果 |
|---|---:|---|
| `cargo test -p alephcore --lib gateway::channel_registry::tests::` | 0 | 63 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::webhook_receiver::tests::` | 0 | 22 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::delivery_queue::tests::` | 0 | 52 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib gateway::channel_health_monitor::tests::` | 0 | 14 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib tools::registry::tests::` | 0 | 54 passed, 0 failed, 0 ignored |
| `cargo check -p alephcore --tests` | 0 | passed |
| `cargo test -p alephcore --lib gateway::interfaces::webhook::` | 0 | 51 passed, 0 failed, 0 ignored |
| `cargo test -p alephcore --lib capability::` | 0 | 222 passed, 0 failed, 0 ignored |
| `cargo fmt --all --check` | 0 | passed |
| `git diff --check` | 0 | passed |

An earlier attempted multi-filter Cargo invocation was rejected before test execution (`error: unexpected argument ...`); it is not counted as a test result. A shell command that could not find a suite log also stopped before Cargo and is not counted as a test result.

一次多 filter Cargo 调用在测试执行前被参数错误拒绝；另一次 shell 因找不到 suite log 在 Cargo 前停止。两者均不计为测试结果。

## Review / 审查

Independent reviewer `del_mvri61gw_y936` failed before review with exit 1 and the exact error `No API key found for anthropic.` No independent approval is claimed. The main agent performed the requested manual read-only checklist and recorded it in `.superpowers/sdd/2026-10-27-gateway-bound-compatibility-closure-plan/final-review.md`; it found no new Stage C Critical/Important defect.

独立 reviewer `del_mvri61gw_y936` 在审查前以 exit 1 失败，错误为 `No API key found for anthropic.`；不声称独立 review 通过。主 agent 已按同一 checklist 完成手工只读审查，记录于上述文件，未发现新的 Stage C Critical/Important 缺陷。

## Additional integration caveat / 额外 integration 保留项

`cargo test -p alephcore --test sms_delivery_policy_test` was run after migrating its fixtures to validated Legacy queue admission. `retirement_is_sticky_after_unregister_and_across_a_restart` passes after fixing fixture registration/marker ordering. Two tests still fail their own 5-second timeout with `no deadlock: Elapsed(())`:

运行 `sms_delivery_policy_test` 前已将其 fixture 迁移到 validated Legacy queue admission。修正注册/marker 顺序后，`retirement_is_sticky_after_unregister_and_across_a_restart` 通过。以下两个测试仍在自身 5 秒超时处失败，错误为 `no deadlock: Elapsed(())`：

- `explicit_only_channel_unregistered_mid_send_is_never_persisted`
- `explicit_only_channel_replaced_mid_send_is_never_persisted`

The Stage C diff does not modify Legacy `send`/`unregister`; these extra failures are not claimed GREEN or fixed. No baseline claim is made without a clean baseline reproduction.

Stage C diff 未修改 Legacy `send`/`unregister`；不声称这两个额外失败已修复或通过，也未在缺少 clean baseline reproduction 时断言其一定为既有问题。

## Files and commits / 文件与提交

Stage C production/test changes are in:

- `src/gateway/channel_registry.rs`
- `src/gateway/channel_registry/binding.rs`
- `src/gateway/delivery_queue.rs`
- `src/gateway/webhook_receiver.rs`
- `src/gateway/inbound_router/executor.rs` (validated Legacy fixture)
- `tests/sms_delivery_policy_test.rs` (validated Legacy fixture)

Relevant commits: `55923b626`, `7252a4910`, `5c683c351`, `3c612c561`, `ae3df80ef`, `66489d94b`.

The raw delivery enqueue is private; the validated Legacy helper is public so external integration fixtures cannot bypass admission. This is not a Bound route.

raw delivery enqueue 已设为 private；validated Legacy helper 为 public，以避免外部 integration fixture 绕过 admission，但它不是 Bound route。

## Unclosed and deferred / 未闭合与延期

First unclosed contract: the two mid-send ExplicitOnly SMS integration behaviors above remain unclosed; independent review also remains unavailable. R1 remains NO-GO/deferred. Stage D, Feishu/R2, all deferred provider/kind integrations, production mount, Zahir route, public Bound send/ingress, EffectClaim, Safe Replay, durable recovery, and scheduler remain deferred.

首个未闭合 contract 是上述两个 mid-send ExplicitOnly SMS integration 行为；独立 review 也仍不可用。R1 继续 NO-GO/deferred；Stage D、Feishu/R2、其余 deferred provider/kind、production mount、Zahir route、public Bound send/ingress、EffectClaim、Safe Replay、durable recovery 与 scheduler 均保持 deferred。

No destructive or external action was performed: no reset/clean, push, deployment, external message, production mount, or external service mutation.

未执行任何 destructive/external action：没有 reset/clean、push、部署、外部消息、production mount 或外部服务变更。
