# Gateway Bound Retirement, Replacement, and Unregister Run Record

**日期 / Date:** 2026-10-27
**范围 / Scope:** Stage B only — source-owned Bound retirement, replacement, unregister, and descriptor/schema/lifecycle-contract publication change.

## Outcome / 结果

Stage B is complete for the bounded Generic Webhook/loopback test composition. The implementation uses the existing `ChannelRegistry` map and entry as the only Gateway association authority. `EntryRuntime.retiring` fences admission and completion during unlocked cleanup. Successor preparation is private; revision capacity is checked before retirement and again before final publication/removal; cleanup and task/forwarder joins happen outside the source gate; the final tuple or removal is committed under one short source-gate section.

Stage B 已在受限 Generic Webhook/loopback 测试组合上完成。实现继续使用既有 `ChannelRegistry` map/entry 作为 Gateway 唯一 association authority。`EntryRuntime.retiring` 在锁外 cleanup 期间 fence admission 与 completion；successor 私下准备；revision capacity 在 retirement 前和最终 publication/removal 前分别 checked；cleanup 与 task/forwarder join 均在 source gate 外执行；最终 tuple 或 removal 在一个短 source-gate 区段内提交。

## Commits / 提交

- `a6d122bf4 test(gateway): define bound retirement contracts` — six contract RED tests and real stop-failure observation support.
- `df513848c feat(gateway): retire bound runtime publications safely` — entry-local retirement, drain, final revalidation, replacement/unregister publication, and stale completion protection.
- `255ebc8a6 test(gateway): observe bound publication boundary` — real loopback publication-boundary observation and cleanup-race coverage.

## RED evidence / RED 证据

Before lifecycle behavior was implemented, each command was preceded by `python qa/browser_live/check_memory.py`, which exited `0` with `ALLOW`.

| Command | Actual result |
|---|---|
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_retirement_discards_original_completion -- --exact` | Exit `101`; `running 1 test`, `0 passed; 1 failed`; old completion was still accepted. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_replace_publishes_whole_tuple -- --exact` | Exit `101`; `running 1 test`, `0 passed; 1 failed`; replacement returned `Err(Busy)`. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_unregister_waits_for_cleanup -- --exact` | Exit `101`; `running 1 test`, `0 passed; 1 failed`; unregister returned before cleanup. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_contract_update_retires_old_publication -- --exact` | The first attempt exposed a test setup error by resolving a Cold provider; after starting the incumbent, rerun exited `101`, `0 passed; 1 failed`, with the scaffold returning `Err(Busy)`. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_failed_cleanup_retains_incumbent -- --exact` | Exit `101`; `running 1 test`, `0 passed; 1 failed`; cleanup failure was not yet a `Runtime` residual failure. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::bound_revision_exhaustion_never_half_publishes -- --exact` | Exit `101`; `running 1 test`, `0 passed; 1 failed`; exhaustion was not yet rejected. |

All six failures were contract RED after parser/import/type issues and compile scaffolding were repaired. The known nonfatal warning appeared on cargo test builds: `ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`.

六个失败均为 contract RED；parser/import/type 与 compile scaffolding 问题已先修复。cargo test 构建中出现已知非致命 warning：`ld: __eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table`。

## Implementation / 实现

- `src/gateway/channel_registry.rs`: extracted shared Bound entry preparation; first registration and replacement use the same construction path.
- `src/gateway/channel_registry/binding.rs`: added entry-local `retiring`, checked replacement/unregister operations, retirement marker, unlocked drain/join/cleanup, final revalidation, atomic publication/removal, and source → ownership tree → provider → original entry/ticket completion validation.
- `src/gateway/channel_registry/test_support.rs`: added a real `FailingStopWebhook` wrapper that delegates to `WebhookChannel` and fails at the channel stop boundary.

`ProviderHandle` remains the sole provider instance/state/generation authority. `OwnershipTree` remains the sole owner/lifetime/generation/revoke/dispose authority. No parallel registry, owner map, descriptor catalog, runtime table, `CapabilityKind`, generalized envelope, or durable store was added.

## GREEN evidence / GREEN 证据

Every cargo command below had a preceding successful `python qa/browser_live/check_memory.py`; all tests had zero failures. The known macOS linker warning above was the only nonfatal warning observed on test builds.

| Command | Actual result |
|---|---|
| Six exact Stage B tests listed in the RED table | Each exited `0`: `running 1 test`, `1 passed; 0 failed; 21488 filtered out`. |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::` | `59 passed; 0 failed; 0 ignored; 21430 filtered out`. |
| `cargo test -p alephcore --lib gateway::channel_registry::binding::tests::` | `3 passed; 0 failed; 0 ignored; 21486 filtered out`. |
| `cargo test -p alephcore --lib gateway::interfaces::webhook::` | `51 passed; 0 failed; 0 ignored; 21438 filtered out`. |
| `cargo test -p alephcore --lib capability::` | `222 passed; 0 failed; 0 ignored; 21267 filtered out`. |
| `cargo check -p alephcore --tests` | Exit `0`; finished successfully. |
| `cargo clippy -p alephcore --tests -- -D warnings` | Exit `0`; finished successfully with no clippy errors. |
| `cargo fmt --all --check` | Exit `0`. An intermediate formatting check exited `1`; formatting was applied and the final check passed. |
| `git diff --check` | Exit `0`. |
| Final publication-boundary focused test after the last test strengthening | `1 passed; 0 failed; 21488 filtered out`. |

## Review / 审查

An independent read-only Pi reviewer was attempted with the final review package:

`/Volumes/TBU4/Workspace/Aleph/.superpowers/sdd/2026-10-27-gateway-bound-retirement-plan/review-a6d122bf4..255ebc8a6.diff`

It was unavailable and did not produce a review result. Exact failure:

`Codex error: {"error":"acp-proxy failure","detail":"Error: upstream circuit open: chatgpt.com: 3 consecutive pre-response failures since 2026-10-27T08:37:36.114Z · last failure connect-timeout ... endpoint unreachable","status":502}`

独立 reviewer 不可用，不能记为通过。主 agent 按相同 checklist 完成手工审查：half-publication、source-gate/I/O lock discipline、stale completion reaching successor、revision exhaustion before and after unlocked cleanup、failure residual truth、authority duplication，以及 Legacy/Bound compatibility path。审查未发现新增 authority、锁内真实 I/O、successor textual-ID completion mutation 或 deferred subsystem scope creep。

## First unclosed contract and deferred scope / 首个未闭合契约与 deferred 范围

The first unclosed contract is broader than this bounded slice: R1's complete resource/compatibility matrix and production runtime mount remain unproven. This Stage B result does not establish clustered ownership, crash recovery, durable replay, external exactly-once effects, or all-channel lifecycle isolation.

R1 仍为 NO-GO/deferred。Feishu/R2、production Bound opt-in/mount、Zahir route、public send/ingress、EffectClaim producer、Safe Replay、external exactly-once、durable/crash recovery、universal scheduler、remaining provider kinds、terminal/memory/sandbox migration，以及完整 all-channel resource matrix 均保持 deferred。

## Not run / 未运行验证

- Full workspace `bun run check` was not run; the required `alephcore` test/check/clippy/fmt gates were run.
- Stage C compatibility closure, Stage D real resource matrices, live production mount, Zahir/Feishu integration, crash/recovery tests, and external provider tests were not run.
- No independent reviewer result was obtained because the reviewer service was unavailable.

## Destructive or external actions not taken / 未执行的 destructive 或 external action

No production mount, deployment, external message send, Feishu/R2 wiring, Zahir route, database migration, durable-store write, force push, or remote publication was performed. The commits above are local repository commits only.

没有执行 production mount、deployment、external message send、Feishu/R2 wiring、Zahir route、数据库迁移、durable-store 写入、force push 或 remote publication。上述提交仅是本地仓库提交。
