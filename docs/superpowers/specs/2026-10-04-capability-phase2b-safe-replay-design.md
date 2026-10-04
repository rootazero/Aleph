# Capability Phase 2B — Restricted Safe Tool Replay

> Status: design review required; Phase 2B is not implementation-ready until the claim, input, snapshot, and per-call crash-budget contracts below are accepted.
>
> Date: 2026-10-04
>
> Parent: `docs/superpowers/specs/2026-10-04-capability-phase2-durable-tool-design.md`
>
> 本规格定义 Phase 2B 的保守边界：受限、可审计的 at-least-once replay。它不承诺 exactly-once，也不把 `ReplayDecision::AutoReplayEligible` 当作外部副作用授权。

## 1. Goal

Phase 2A already persists the call-time descriptor identity and computes a fail-closed replay classification. Phase 2B adds one narrowly scoped execution path for a dangling Safe Tool call:

1. prove the stored call and the current callable contract have the same replay contract token;
2. capture the handler and descriptor from one immutable registry snapshot;
3. execute under the existing `ResumeCoordinator` session claim;
4. append the real `ToolResult` or `ToolError` using the original `call_id`;
5. keep unresolved calls unknown and bounded when the process crashes again.

The result is intentionally **at-least-once**. If the external effect completes but the outcome append does not, a later resume may invoke the handler again. The Safe declaration and tool-specific contract must therefore tolerate duplicate invocation. No component may describe this path as exactly-once.

中文目标：2B 只为已证明的 Safe dangling call 增加一个受限恢复执行入口。执行必须绑定同一 registry snapshot，并复用现有 session claim；结果使用原始 `call_id` 写回。effect 与 outcome 落盘之间仍存在 crash window，因此语义是 at-least-once，而不是 exactly-once。

## 2. Non-goals and hard boundaries

- No general durable Task Scheduler.
- No second Tool registry, replay registry, or external-effect ledger.
- No new wire protocol message.
- No replay for legacy events whose identity lacks the new contract token.
- No inference from `idempotent`, `concurrent_safe`, tool name, source, or registry ordering.
- No automatic replay from team/cron/heartbeat `repair_and_close_abandoned` paths in this phase; those paths remain VerifyOnly until they can supply the same execution truth and claim contract.
- No replay logic or platform API calls in `src/harness/`.
- No claim of exactly-once, transactional coupling between an external effect and SQLite, or universal idempotency.

## 3. Terminology

### 3.1 Replay contract token

`ToolCallIdentity` gains an optional fixed-size `replay_contract_fingerprint`. It is present for a newly emitted Safe identity and is compared byte-for-byte during recovery. The token must be stable across process restart and independent of registration order or the in-memory registry revision.

The fingerprint is a contract token, not a proof of handler machine code. For a Safe tool, registration must provide an audited implementation/behavior identity and bump it whenever the handler's externally observable replay contract changes. The canonical descriptor fields are included in the token calculation so schema and policy changes cannot reuse an old token accidentally. A Safe descriptor without an explicit valid token is rejected at registration or treated as VerifyOnly; it is never replayed.

Recommended token input, serialized with a versioned canonical encoding:

- fingerprint algorithm/version;
- tool name and `ToolKind`;
- descriptor `schema_version`;
- canonical JSON input schema;
- source identity (`Builtin`, MCP server id, or extension/plugin id);
- replay policy and confirmation contract;
- the audited implementation contract id/version supplied by the Safe registration owner.

Do not include the process-local `revision`. Do not hash arbitrary serde map order; canonicalize JSON object keys before hashing. The implementation should use the already available SHA-256 dependency and a lowercase fixed-length representation or fixed byte array with an explicit version. Legacy identity data without this field remains valid session data but is never 2B-replayable.

`revision` remains in the identity as a short-lived in-process TOCTOU guard. It is no longer sufficient evidence by itself and must not be described as a durable generation.

### 3.2 Replay classification versus authorization

`ReplayDecision` remains a classification result. It must not be `Copy`-converted into a permission, passed as a boolean, or directly invoke a Tool.

The execution path creates a private, non-`Copy` replay permit only after all of these hold:

- the durable session recovery claim is held;
- stored identity and current descriptor both declare `Safe`;
- schema version and contract fingerprint match;
- the paired immutable registry state is open and contains the named entry;
- the state-level generation is non-reused and the Phase 2A revision equality guard also matches where applicable;
- the handler and descriptor were captured together from that same state;
- the call is still structurally dangling under a fresh reduction with one original dispatch, no duplicate contradiction, and proven effective input.

The permit carries the captured handler `Arc`, descriptor identity, original `call_id`, tool name, and the recovery claim context. It is consumed once by the replay executor. It cannot be constructed by a caller from `ReplayDecision` or from a boolean.

## 4. Execution architecture

### 4.1 Single production seam

The only execution seam is the existing session recovery operation represented by `repair_boundary_with_policy(...)`, invoked from the interrupted-run branch of `ResumeCoordinator`. The current Phase 2A order is not sufficient: it repairs dangling calls before stamping `ResumeAttempted`, and the coordinator later retriggers the whole run. Phase 2B must first change the recovery contract to this order:

1. obtain a durable, expected-head recovery claim using the existing `ResumeAttempted` event and a narrow store/actor compare-and-append precondition;
2. reduce the post-claim log and validate that each candidate has one original request, no outcome, no duplicate contradiction, and an input-equivalence proof;
3. prepare a private permit from one immutable handler+descriptor snapshot;
4. persist the per-call attempt cursor/budget before that call's effect;
5. invoke one eligible call and append its real outcome using the original `call_id`;
6. reduce again before selecting the next dangling call;
7. handle VerifyOnly calls with the existing non-replay repair policy;
8. only after the repair/replay result is durably known, continue to the existing whole-run retrigger.

The existing Phase 2A implementation currently does not satisfy this order. No 2B execution code may be added until the store claim/outcome precondition and the per-call cursor contract are implemented and tested.

`boundary_repair` owns session facts and event ordering. It receives a narrow, replay-specific command/adapter; it must not import `ToolHandlerRegistry`, `ScopedToolService`, MCP clients, or platform APIs. VerifyOnly repair and replay execution are separate commands/types: `repair_and_close_abandoned(...)` receives no replay adapter and cannot obtain a permit. The gateway/startup layer adapts the current registry snapshot and execution bridge to the replay command. This preserves the core/bridge boundary.

### 4.2 Snapshot and TOCTOU rule

The adapter must read one immutable registry state containing the paired handler and descriptor, plus the state-level generation/closed status. It must not call `tool_call_identity(name)` and later resolve `name`, nor read a live registry status after capturing the snapshot. Permit creation is entirely derived from that one state; replacement/unregister/close after permit creation does not swap the captured handler, but the permit remains bounded by the recovery claim and is consumed once. A registry implementation may expose a single atomic preparation method instead of exposing raw state. A process-local revision is only an additional equality check and is never the cross-restart proof.

### 4.3 Input and outcome shape

Replay may use an input only when the durable request proves it is the same input that reached the handler. Current Phase 2A emits `ToolCallRequested` before guardrail sanitization in `src/harness/agent/act.rs:484-509`; therefore a legacy/current request whose input may have been transformed, blocked, or otherwise diverged is VerifyOnly for 2B. Phase 2B must either persist the effective post-guardrail input as part of the durable intent before execution, or carry an explicit non-replayable marker. A blocked call must never be revived by replay.

When input equivalence is proven, replay uses that effective input and the original `call_id`. It does not append a second `ToolCallRequested`, so prompt projection contains one original request. A successful invocation appends a real `SessionEvent::ToolResult` containing the actual `ToolOutput`. An invocation error appends the existing `SessionEvent::ToolError` shape with the original call id. A synthetic unknown-outcome `ToolError` is reserved for VerifyOnly and must not be used after an effect has started but its outcome commit is unknown.

The replay adapter may pass `call_id` to tracing and audit hooks. The current `ToolHandler::invoke(input)` API has no call-id argument; 2B must not silently claim handler-level dedup exists. The same `call_id` is correlation identity, not a deduplication key. A future handler API may add explicit replay context, but that is separate from this at-least-once contract.

### 4.4 Durable claim and outcome precondition

The current `load_head_seq() + append()` sequence is not a cross-process claim. Before implementation, `SessionEventStore` must provide a narrow expected-head/compare-and-append or equivalent single-owner actor operation for the recovery stamp and each outcome. The operation must prove that the target call is still unanswered and that no contradictory duplicate dispatch/receipt exists before writing the claim/outcome. A process-local `ResumeSlot` alone is insufficient.

A pre-commit append failure leaves the call dangling. A post-commit-unknown result must not be answered with a synthetic error; the next reduction treats the call as unknown and the at-least-once contract permits a retry. Only a confirmed handler error with a confirmed durable `ToolError`, or a confirmed output with a confirmed durable `ToolResult`, answers the call. Replay outcomes must use Barrier durability unless the store contract proves an equivalent recovery guarantee.

## 5. Crash and concurrency semantics

### 5.1 Existing claim and per-call budget

`ResumeCoordinator::try_claim_resume(...)` / `ResumeSlot` remains the process-local admission guard, but it is not a distributed claim. The durable recovery claim must use an expected-head/serialized store operation. The existing `ResumeAttempted` event currently has only `target` (the resumed `RunStarted` or unanswered `UserMessage`) and `attempt`; Phase 2B must not overload `target` to mean a call. It must either extend this existing event compatibly with an optional per-call cursor/call identity, or use equivalent durable store metadata without adding a new protocol event. The chosen form must validate the target run and call. One batch stamp for an entire dangling set is not enough to prove a per-effect retry bound.

Phase 2B must persist a per-call cursor or equivalent attempt budget before each effect. The contract must define the budget unit and crash-after-index-k recovery. A safe implementation may use one bounded stamp/cursor per dangling call; it must not claim that `max_attempts` on a whole resume invocation bounds every effect. Once a call's budget is exhausted, it becomes VerifyOnly/unknown and no synthetic success is allowed.

### 5.2 Effect/outcome window

The following sequence is explicitly accepted:

1. intent is durable;
2. a per-call replay attempt is durable;
3. replay handler performs an external effect;
4. process dies or outcome commit becomes unknown;
5. next reduction sees the call as unanswered and may invoke it again, within that call's remaining budget.

The system must record non-sensitive structured diagnostics such as `effect_started`, `outcome_committed`, or `outcome_commit_unknown`; it must not put storage errors, raw input, or external payloads into a model-visible error. Safe registration is an explicit owner obligation to tolerate this duplicate-effect tradeoff.

No at-most-once claim is made either. The effect invocation count and durable receipt count can differ under crash or concurrent-writer failure. This is at-least-once execution, not exactly-once and not a deduplication guarantee.

### 5.3 Multiple dangling calls

Replay calls are serialized in original event order under the same durable recovery claim. Before each permit, the fresh reduction must prove one original dispatch, no outcome, no duplicate contradiction, and input equivalence. After each confirmed outcome append, the next reduction selects the remaining set. If a later call crashes, earlier confirmed outcomes remain durable and are not replayed again.

## 6. Eligibility and fail-closed matrix

| Condition | 2B result |
|---|---|
| Legacy/malformed/missing identity or fingerprint | VerifyOnly |
| Stored or current policy is `Unsafe` | VerifyOnly |
| Missing current descriptor/handler | VerifyOnly |
| Schema version mismatch | VerifyOnly |
| Contract fingerprint missing or mismatched | VerifyOnly |
| Registry state is closed or paired entry is absent | VerifyOnly |
| State-level snapshot does not satisfy the stored contract | VerifyOnly |
| Input was sanitized/blocked after the durable request, with no effective-input proof | VerifyOnly |
| Duplicate dispatch/receipt contradiction | VerifyOnly |
| Call already has `ToolResult`/`ToolError` | Not dangling; do not execute |
| Durable claim or expected-head append unavailable | VerifyOnly/refuse; do not execute |
| Handler returns an error and its outcome commits | Append real `ToolError` with original call id |
| Handler returns output and its outcome commits | Append real `ToolResult` with original call id |
| Process dies before outcome commit | Retry only within that call's persisted budget; then VerifyOnly |

No failure in this table may fall through to “assume Safe”.

## 7. Safe registration contract

`ReplayPolicy::Safe` is an explicit recovery contract, not a synonym for `ToolDefinitionMetadata::idempotent`. Before a descriptor can be registered as Safe, its owner must provide:

- the stable replay contract token;
- the external-effect idempotency/duplicate-effect analysis;
- the behavior when the outcome is unknown;
- the maximum acceptable retry count under the existing recovery ratchet;
- evidence that the tool does not require a fresh interactive approval during unattended recovery.

MCP and extension tools remain Unsafe by default. A future Safe external tool must pass the same registration review; source type alone never grants eligibility.

## 8. Verification contract

Before implementation is accepted:

1. Cross-restart test: identical Safe descriptor and implementation token across fresh registry instances remains eligible; a reset local revision alone cannot authorize a changed contract.
2. Contract-change test: changing input schema, source identity, replay policy, or implementation token becomes VerifyOnly even if local revisions repeat.
3. Snapshot test: all permit inputs come from one immutable state; replacement/close after permit creation cannot swap the invoked handler.
4. Input-equivalence test: pre-sanitize requests and blocked calls are VerifyOnly; only durable effective input may be replayed.
5. Claim test: competing coordinators cannot both acquire the expected-head recovery claim or append an outcome for the same unanswered call.
6. Outcome test: a successful recovery executor may issue one outcome attempt under the claim, but crash/concurrent-writer conditions may produce zero or duplicate receipts; this is not exactly-once.
7. Partial-batch test: earlier confirmed outcomes survive a later failure; a fresh reduction excludes answered calls.
8. Per-call crash-budget test: injected failure after effect and before outcome is retryable only within that call's persisted budget; crash-after-index-k behavior is explicit.
9. Legacy and malformed identity tests remain fail-closed.
10. Delegated path test: `repair_and_close_abandoned(...)` has no replay command/permit and invokes no handler.
11. Architecture test/review confirms no `ToolService` or platform API dependency enters `src/session` or `src/harness`.
12. Existing Phase 2A event, reduction, boundary, registry, recovery, and harness tests remain green except previously recorded baseline failures.

## 9. Implementation gate and acceptance statement

Phase 2B is not implementation-ready until the plan adds and verifies:

- the existing `ResumeAttempted` event is extended compatibly with a per-call cursor/call identity, without introducing a new protocol event, or an equivalent durable store-side cursor is provided;
- a narrow durable expected-head/serialized claim and outcome precondition rather than relying on `load_head_seq() + append()`;
- a per-call cursor or budget with crash-after-index-k semantics;
- a single-state registry preparation API containing paired handler, descriptor, closed status, and non-reused state generation;
- an effective-input proof or an explicit VerifyOnly result for requests affected by guardrails/sanitization;
- an API-level separation in which delegated VerifyOnly repair cannot receive a replay executor.

After those prerequisites are accepted, Phase 2B is complete only when the implementation demonstrates bounded at-least-once effect invocation and documents the duplicate-effect tradeoff. It must distinguish three counts: original `ToolCallRequested` records, external effect invocations, and durable outcome receipts. The same `call_id` is correlation identity, not deduplication. None of these requirements provides exactly-once behavior.

严格验收标准：必须证明 fingerprint 跨重启稳定、handler snapshot 与 descriptor 同代、effective input 可证明、durable claim 与 outcome 前提成立、每个 call 的 crash budget 有界、outcome 未落盘时可能重复执行这一事实被明确记录，并且 delegated 路径没有获得 replay 能力。
