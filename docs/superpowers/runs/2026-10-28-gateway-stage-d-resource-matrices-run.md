# Gateway Stage D — Source-Owned Resource Matrices / 源所有资源矩阵

- Date / 日期：2026-10-28
- Branch / 分支：`stage-d-resources`
- Base / 基线：Stage C `47625530cbc26da41a791caa09b8c7cfd0b485d3`
- Implementation HEAD / 实现 HEAD：`a741ac2c549307c62962fa2187712d7ead8a51ad`
- Status / 状态：**bounded Stage D closed / 限定范围 Stage D 已闭合**

## Scope and decision / 范围与决策

Stage D proves the existing source-owned lifecycle with one real Generic Webhook adapter and a second test-only polling/socket shape. Approach A was selected: test-only fixtures and observation hooks beside `ChannelRegistry`, reusing the real registry, `ProviderHandle`, `OwnershipTree`, source revision and retirement APIs. No production polling provider or second lifecycle/resource authority was added.

Stage D 用一个真实 Generic Webhook adapter 和第二个 test-only polling/socket 形态验证既有 source-owned lifecycle。采用方案 A：在 `ChannelRegistry` 旁增加测试 fixture 与观测钩子，复用真实 registry、`ProviderHandle`、`OwnershipTree`、source revision 和 retirement API；没有新增生产 polling provider 或第二套 lifecycle/resource authority。

Implementation commits / 实现提交：

- `f59f1ba52` real polling/socket fixture and positive controls / 真实 fixture 与 positive controls
- `1d25d9fa1` cold/active/replacement matrices / cold、active、replacement 矩阵
- `8cd95d0ce` bounded observations and stale-generation assertions / 有界观测与 stale-generation 断言
- `466b2a8ce` activation failure and failed-stop residual evidence / activation failure 与 failed-stop residual 证据
- `a741ac2c5` real post-open SQLite schema failure and final review hardening / 真实 open 后 schema failure 与最终复审加固

## Real resources and limits / 真实资源与限制

| Shape / 形态 | Real observed provider resources / 实际观测的 provider 资源 | Explicit limits / 明确限制 |
|---|---|---|
| Generic `WebhookChannel` | `reqwest::Client` handle construction/release; real loopback HTTP request/response / Client 句柄构造与释放、真实 loopback HTTP 请求/响应 | `client_drop` means the owning channel released its `Option<Client>` handle, not that every internal reqwest pool task was joined. Webhook binds no provider socket. / `client_drop` 不代表 reqwest 内部 pool task 全部 join；Webhook 不绑定 provider socket。 |
| Test-only `PollingSocketChannel` | Loopback TCP connect/drop and peer EOF; tracked tokio worker cancellation/join; interval ticks/drop; fixture cache; temporary SQLite open/close/drop / TCP 连接/释放与对端 EOF、worker cancel/join、timer、cache、临时 SQLite | `CACHE_*` observes a fixture-owned `Vec<u64>`, not a Core cache. Polling send records payload only and performs no external I/O. One fresh DB path is used per activation; same-file restart is not a supported fixture case. / cache 只是 fixture `Vec<u64>`；send 无外部 I/O；每次 activation 使用新 DB 路径。 |

The HTTP/TCP peer and pre-created SQLite schema are test infrastructure, not provider allocation. Websocket, provider listener, OS thread and token-client authentication rows are N/A for these shapes; no such resource is inferred from TCP/HTTP success. Registry entries, descriptors and `ProviderHandle` are explicitly excluded from the zero-resource delta.

HTTP/TCP peer 和预建 SQLite schema 属于测试基础设施，不计入 provider allocation。websocket、provider listener、OS thread、token-client authentication 对这两个形态为 N/A；registry entry、descriptor 和 `ProviderHandle` 不计入 provider 零资源增量。

## Executable contracts / 可执行契约

Six tests in `src/gateway/channel_registry/resource_matrix.rs` cover:

1. `active_resource_observers_have_positive_controls`: nonzero real receipts before cleanup, so an inactive observer cannot manufacture a zero.
2. `cold_operations_allocate_no_resources_for_either_shape`: two shapes × Discovered/Cold/Disabled × correct/wrong/missing ownership and scope. Register/list/status/health/snapshot/capture/resolve/rejected reads/unregister allocate no provider resources. Correct-binding cold resolve fails provider admission because the provider is not Ready.
3. `active_send_stop_and_old_proof_contracts_hold_for_either_shape`: activation, shape-specific observed send, stop cleanup and stale-proof rejection.
4. `replacement_cleans_old_resources_and_rejects_old_provider_proof`: actual `replace_bound_channel`, independent new receipts, stale entry/generation rejection and successor positive/cleanup controls.
5. `start_failures_do_not_fabricate_resources_or_cleanup`: invalid Webhook config before client attempt; closed-port connect; valid TCP followed by SQLite open failure with peer-observed EOF; successful SQLite open followed by real schema failure with `DATABASE_OPENED` and `DATABASE_DROP` both observed.
6. `failed_stop_preserves_real_residual_until_owner_drop`: failed stop retains the incumbent and client handle; retry and unregister remain `Busy`; owner drop releases the handle exactly once. `transport_may_be_live` is only a conservative diagnostic, not proof by itself.

六个测试覆盖 positive controls、双形态三初态 cold 矩阵、active/send/stop/old-proof、真实 replacement、四类 start failure 以及 failed-stop residual。冷读取不激活 provider；failed-stop 没有被改成 retry 成功，owner drop 只声称释放 channel 持有的 client 句柄。

## Behavioral RED / GREEN evidence / 行为 RED/GREEN 证据

- Task 1 inactive polling fixture failed the positive-control assertion (`tcp_connect_attempt` 0 vs 1), then passed after real resource implementation.
- Task 2 initially reused a cold-generation proof after activation; the test failed on the actual stale generation and was corrected to reject that old proof explicitly while capturing a fresh active proof for the valid send.
- Task 3 failed-stop initially observed owner `client_drop == 0` against expected 1; after the `#[cfg(test)]` owner-drop observation it passed. The final schema-failure scenario also caught a missing moved receipt during development; `OwnedDatabase::new` now records open before schema execution and the guard records destruction on failure.
- Review-driven schema setup initially used `CREATE TABLE IF NOT EXISTS`, which did not fail against the prepared object. The fixture now uses a fresh DB per activation and real `CREATE TABLE`; the prepared-table case reaches the intended post-open schema failure without weakening assertions.

详细任务报告位于 ignored SDD ledger；最终验证日志位于 `.superpowers/sdd/2026-10-28-gateway-stage-d-resource-matrices-plan/task-4-verification/`。最终三次矩阵日志 `final-matrix-repeat-{1,2,3}.cargo.log` 均含 6/6 通过，第一份包含最终提交字节的实际 `Compiling alephcore` 行。

## Final verification / 最终验证

Every Cargo invocation had its own `python qa/browser_live/check_memory.py` gate with exit 0 / `ALLOW`. No zero-test filter was accepted. Final results at `a741ac2c5`:

每条 Cargo 命令均单独通过 memory gate（exit 0 / `ALLOW`）；最终 HEAD 的结果为：

| Command / 命令 | Result / 结果 |
|---|---|
| Two focused Legacy ExplicitOnly mid-send tests, three serial runs each / 两个 mid-send 各三次 | 6 invocations, each `1 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::` | `6 passed; 0 failed; 0 ignored`; three additional serial repeats also 6/6 |
| `cargo test -p alephcore --test sms_delivery_policy_test` | `18 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::channel_registry::tests::` | `63 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::webhook_receiver::tests::` | `22 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::delivery_queue::tests::` | `52 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::channel_health_monitor::tests::` | `14 passed; 0 failed` |
| `cargo test -p alephcore --lib tools::registry::tests::` | `54 passed; 0 failed` |
| `cargo test -p alephcore --lib gateway::interfaces::webhook::` | `51 passed; 0 failed` |
| `cargo test -p alephcore --lib capability::` | `222 passed; 0 failed` |
| `cargo check -p alephcore --tests` | exit 0 |
| `cargo clippy -p alephcore --tests -- -D warnings` | exit 0 |
| `cargo fmt --all --check` | exit 0 |
| `git diff --check` | exit 0 |

Only the known nonfatal macOS linker warning `__eh_frame section too large` appeared. 仅出现既有非致命 linker warning。

## Independent review / 独立审查

Fresh whole-branch reviewer `del_mvs1alzo_wzs8` returned **Ready: Yes**, zero unresolved Critical/Important, after inspecting the full diff and raw verification logs. Scoped final re-review `del_mvs2i9s8_tbjc` at `a741ac2c5` also returned **Ready: Yes**, confirming real schema-failure coverage and resolution of the residual/receipt findings. Earlier per-task reviews also passed after their findings were fixed; watchdog-terminated runs without verdicts were not counted as independent approval.

独立全量 reviewer 与最终 scoped re-review 均为 Ready Yes，无未解决 Critical/Important。没有 verdict 的 watchdog 终止运行不计为独立通过。

## Remaining limits and non-work / 剩余限制与未做项

- Stage D is bounded resource evidence for two shapes, not production polling, all-channel isolation, process crash isolation or executable unloading.
- Failed-stop retry/recovery remains the existing `Busy` contract; `mount_retained` remains an unassigned production field and is not claimed as tested.
- Reqwest pool internals, registry-internal allocations, all production social providers, terminal, memory graph and sandbox remain unproven/deferred.
- Runs are Darwin arm64 only. Windows/CI-host behavior, loaded-machine timing and the memory-gate implementation itself were not independently validated.
- Not run: full unfiltered `alephcore --lib`, workspace-wide tests, bins/shared-surface suites, `qa/channels`, external-network/provider tests or production mount. Only the specified focused and Stage C regression sets were run.
- R1 remains separately NO-GO/deferred. Stage E, Feishu/R2, production Bound opt-in/mount/Zahir, public send/ingress, EffectClaim, Safe Replay, durable scheduler and other provider/kind integrations remain unauthorized.

本阶段没有执行 merge、push、worktree cleanup、reset/clean、deploy、外部消息或服务变更；也没有进入 Stage E 或生产组合能力。
