# Capability Phase 2 — Durable Tool Intent and Safe Replay Design

> Status: approved design; Phase 2A implementation complete in this worktree. Phase 2B remains separately gated.
>
> Date: 2026-10-04
>
> Parent architecture: `Everything-externally-composable-is-a-Capability.md`
>
## 1. Goal and problem statement

第一期建立了 Tool Capability 的 descriptor 主链：`ToolHandlerRegistry` 是 callable truth，handler 与 descriptor 通过同一 generation 原子投影，descriptor 能投影到 provider metadata、ToolCatalog、MCP adapter 和 execution index。第一期留下的明确缺口是：`SessionEvent::ToolCallRequested` 没有保存调用发生时的 descriptor 身份，因此 recovery 只能读取当前 descriptor，无法证明当前实现就是崩溃前调用的那一代。

本期目标是让一次外部可组合 Tool 调用拥有可持久化的 durable intent 身份，并使恢复资格可由事件日志与当前 descriptor 双向证明。目标不是复制 Pi Durable 的通用 Task Scheduler，而是把 Pi Durable 的 intent checkpoint、replay policy 和 call idempotency 映射到 Aleph 已有的 SessionEventStore、Barrier durability、RunReduction、boundary repair 和 ResumeCoordinator。

## 2. User-facing outcome

当进程在 Tool intent 已提交之后、Tool outcome 提交之前崩溃时，Aleph 必须能区分：

- 旧日志缺少新字段：安全地视为未知结果，继续现有 fail-closed repair；
- 调用时或当前 descriptor 不允许 replay：安全地视为未知结果，不自动重放；
- descriptor 已被替换或 schema generation 不匹配：安全地视为未知结果，不使用新 handler 重放旧调用；
- 两阶段实现都证明同一 Safe descriptor generation 且本期执行入口允许 replay：才能产生 `AutoReplay` 资格。Phase 2A 只计算并记录资格，不执行外部效果；Phase 2B 才把资格接入实际执行路径。

任何“结果未知”都不得被消费为成功或失败许可。未知外部效果不能自动重试。

## 3. Scope split

### 3.1 Phase 2A — Durable Tool Intent Contract

本阶段实现并验证：

1. 在 `SessionEvent::ToolCallRequested` 增加向后兼容的调用时 descriptor identity。最小字段为 `schema_version`、`revision`、`replay_policy`；具体字段采用已有事件的 serde 兼容模式，并保持旧事件可解码为 `None`。
2. 生产 Tool request 的两个现有路径（`src/harness/agent/act.rs` 的 serial 与 parallel pre-emit）在构造事件前，从已注入的 Tool capability lookup 捕获 descriptor identity，写入同一 `ToolCallRequested` Barrier。这里允许极薄的 harness wiring 修改：它只搬运已解析的数据，不决定 replay，也不访问平台 API。
3. `RunReduction`/dangling call 保留该 identity，避免 recovery 再按工具名猜测。
4. 增加纯资格判定：调用时 policy 为 `Safe`、当前 descriptor policy 为 `Safe`、schema version 兼容、revision 相同且 call id 没有 outcome，才能返回 `AutoReplayEligible`；否则返回现有 `VerifyOnly`/unknown-outcome repair。
5. 让恢复资格判定复用 `ReplayPolicyLookup` 和 `ResumeCoordinator` 现有串行恢复入口，不创建第二个 scheduler、registry 或全局隐式 Tool service。
6. 增加事件 encode/decode、旧日志、descriptor replacement、unknown policy、重复 recovery 和 barrier 顺序测试。

Phase 2A 不调用 Tool，不写新的 `ToolResult`/`ToolError`，不改变外部副作用行为。

### 3.2 Phase 2B — Safe Tool Replay

Phase 2B 是独立审批和实现阶段，不由本规格自动授权。它必须在 2A 证据通过后另写 implementation contract，至少解决：

- `AutoReplayEligible` 的唯一执行入口与 `ResumeCoordinator` 的并发占位；
- 原始 `call_id` 的幂等使用与 outcome 去重；
- handler 执行期间崩溃、结果提交失败、重复 resume 的行为；
- replay 产生既有 `ToolResult`/`ToolError` 配对，且 prompt projection 不出现第二个 tool use；
- Tool execution 不进入 `src/harness/`；但两个真实事件生产点允许纯 wiring 修改，把已解析的 descriptor identity 搬运进事件，不在 harness 中实现 replay 资格、恢复策略或外部副作用。
- Safe 资格在调用、当前 registry generation、恢复入口三处均不可被放宽。

## 4. Mapping from references

| Reference mechanism | Aleph mapping | Not copied |
|---|---|---|
| Pi Durable ToolTask call/execute checkpoints | `ToolCallRequested` Barrier followed by existing outcome events | 通用 pending/running/waiting/completing scheduler |
| Persisted replay policy | call-time fields on `ToolCallRequested` | 从 `idempotent` 或工具名推断 replay |
| Current policy check | `ReplayPolicyLookup` over current `ToolCapabilityDescriptor` | 第二套 descriptor registry |
| call idempotency | existing `call_id` plus existing session reduction/outcome pairing | 新建 external-effect database |
| atomic commit rule | existing `SessionEventStore::write_batch` and Barrier durability | 把所有 Aleph state 合并进单一文档存储 |
| crash recovery | `RunReduction` + `boundary_repair` + `ResumeCoordinator` | 重写 `src/harness/` agent loop |

## 5. Descriptor identity and compatibility contract

### 5.1 Identity

调用时 identity 必须表示“恢复时能否证明是同一 callable contract”，而不只是工具名称：

- `schema_version`: descriptor schema contract version;
- `revision`: registry generation assigned to that descriptor;
- `replay_policy`: explicit declaration copied from descriptor, default `Unsafe`;
- `name` and `call_id` remain the event's existing identity fields.

The event must not persist executable handler code, plugin paths, credentials, or an entire mutable runtime object.

### 5.2 Eligibility

For Phase 2A, eligibility is a pure predicate:

```text
stored call-time replay_policy == Safe
AND current descriptor exists
AND current replay_policy == Safe
AND stored schema_version == current schema_version
AND stored revision == current revision
AND the same call_id has no ToolResult/ToolError outcome
```

The predicate returns a typed result, not a boolean consumed as permission. Missing/invalid/future data returns a fail-closed result. `idempotent`, `concurrent_safe`, source, or name alone cannot make a call replayable.

### 5.3 Old and malformed records

- Legacy `ToolCallRequested` without identity decodes successfully and is `VerifyOnly`.
- Unknown enum values, missing required nested fields, invalid revision or incompatible schema are not replay eligible.
- Serde compatibility must not reinterpret old data as Safe.
- Durable event ordering remains unchanged: `ToolCallRequested` remains a Barrier event.

## 6. Existing architecture and boundaries

- `ToolHandlerRegistry` remains the only callable Tool truth source.
- `entries_snapshot()` is the preferred source for production projection and request identity capture.
- `EffectScope`/`ToolRegistrationScope` remain ownership mechanisms; this phase does not broaden them to all capability kinds.
- `SessionEventStore` remains the durable source; no parallel replay ledger.
- `ResumeCoordinator` remains the only recovery coordinator; no recovery code dispatches directly around it.
- `src/harness/agent/act.rs` 是一个明确限定的 wiring 例外：现有事件生产点必须把已解析的 descriptor identity 搬进 Barrier 事件；不得在 harness 中增加 registry、recovery policy 或 replay execution logic。
- Interface/protocol layers remain projections and do not decide replay policy.

## 7. Failure and concurrency semantics

1. Intent append failure means the Tool is not called; the error is surfaced through the existing call path.
2. A crash after intent and before outcome leaves one dangling call; 2A reports eligibility only.
3. A current registry replacement changes revision; the old call becomes VerifyOnly.
4. Re-running reduction or eligibility is pure and cannot append an outcome.
5. Concurrent recovery must continue to use the existing `ResumeCoordinator` in-flight claim/serialization. Phase 2A adds no second lock with weaker scope.
6. A missing descriptor is unknown, never allow-all.
7. A Safe declaration is not proof of idempotence; it is an explicit recovery contract and must be reviewed at descriptor registration.

## 8. Verification contract

### Phase 2A required evidence

- `cargo check -p alephcore --all-targets --message-format short`.
- Targeted tests for event serde/store round-trip and old event compatibility.
- Targeted tests for reduction identity preservation.
- Targeted tests for Safe/Unsafe/missing/schema mismatch/revision mismatch.
- Targeted tests proving replacement prevents eligibility and repeated eligibility does not append events.
- Existing recovery and registry tests remain green.
- `src/harness/agent/act.rs` 仅增加 serial/parallel 事件字段 wiring；
- `git diff --check` passes and changed files are limited to the approved plan.

Clippy and full lib tests must be run and reported accurately. Existing baseline failures are not silently reclassified as Phase 2 regressions.

### Phase 2B gate

Phase 2B cannot begin from a green unit test alone. It requires a separate design review covering crash windows, duplicate execution, outcome commit failure, and the actual production execution seam.

## 9. Non-goals and deferred work

- No complete Pi Durable Task scheduler.
- No unified Skill/Agent/Plugin/MCP/ACP registry.
- No external-effect ledger.
- No new protocol message type.
- No tool-name-specific recovery branch.
- No automatic replay for legacy events.
- No use of `idempotent` as a replay authorization substitute.
- No changes to `src/harness/` beyond the explicitly scoped serial/parallel event-field wiring required because those are the real `ToolCallRequested` producers.
- No claim that Phase 2B is implemented by adding only a predicate.

## 10. Success criteria

Phase 2A is complete only when the durable identity is emitted at the real production Tool request seam, survives storage round-trip, is preserved by reduction, and drives a fail-closed eligibility result from both stored and current descriptor state. The result must be observable in tests without executing an external Tool. Phase 2B remains explicitly deferred until its execution contract is approved.

## 11. Review questions

1. Is `replay_policy` persisted as an enum or as a compact versioned field, and how does it fail closed for unknown data?
2. Is the existing Tool request seam able to capture the atomic registry entry with only the explicitly scoped wiring change in `src/harness/agent/act.rs`?
3. Is revision equality too strict for any current Tool source, or is it the correct protection against hot replacement?
4. Which existing store test proves old JSON/event rows remain decodable?
5. Can eligibility be represented as a typed result that cannot be accidentally consumed as authorization?
6. Does Phase 2B need a new execution adapter, or can it reuse the current Tool service invocation and session writer?

## 13. Phase 2A verification record

- `CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short`: passed.
- Focused tests passed: events 22, reduction 54, boundary repair 15, descriptor 20, registry 19, durable identity event/store 5, and `harness::tests::act` 27.
- Full `cargo test -p alephcore --lib --message-format short`: `20601 passed; 24 failed; 20 ignored`. The failures are existing platform, environment, configuration, source-census, and timing-sensitive baseline failures; `src/gateway/mcp_face/mod.rs` is an existing scoped-dispatch census failure outside this Phase 2 diff.
- `cargo clippy -p alephcore --all-targets -- -D warnings`: blocked by the existing four `mcp_face` diagnostics: `result_large_err` at `src/gateway/mcp_face/http.rs:133`, `:189`, `:208`, and `large_enum_variant` at `src/gateway/mcp_face/protocol.rs:59`; no new Phase 2 diagnostic was reported.
- `git diff --check`: passed. No `src/harness` file contains recovery or replay logic; the changes there are the declared identity field wiring and fixture compatibility.


Final file list is intentionally deferred to the implementation plan after spec approval. Expected families are:

- `src/session/events.rs`, `src/session/reduction.rs`, `src/session/store.rs` tests;
- `src/session/boundary_repair.rs` and `src/session/mod.rs`;
- `src/harness/agent/act.rs` (the two real Tool request emission points; wiring only);
- `src/tools/descriptor.rs`, `src/tools/registry.rs` tests;
- reference docs and `FEATURE_LOCATOR.md`.

No product code has been changed as part of writing this proposed spec.
