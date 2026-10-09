# H-pre Production Projection Diagnostics Supplement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为已挂载的 H-pre production projection 增加默认关闭、严格本机 operator 授权的诊断控制面，并以真实当前 binary、真实 run-loop 与 recording provider 证明 registry replacement、owner invalidation、真实 lag/overflow recovery 及 close completion。

**Architecture:** 诊断入口只控制已经安装的唯一 `ProjectionHost`、唯一 runtime `OwnershipTree` 和 run-loop 使用的默认 production subscription；不创建第二个 registry、consumer 或 owner authority。入口在启动时仅当 `ALEPH_CAPABILITY_DIAGNOSTICS=1` 条件注册，handler 内直接读取 `CALLER_ROLE`、实际 peer 的 loopback 标记和 connection id，所有缺失或不匹配都 fail-closed。Host 侧提供 status、两个真实 hold plane、release 与 bounded close；现有 source/delivery/applier、bounded replacement、canonical `join_canonical_tools` 和 approval/dispatch 链路继续作为被测路径。

**Tech Stack:** Rust / tokio / serde_json / 现有 `ProjectionHost`、`OwnershipTree`、`ToolHandlerRegistry`、builtin registry、gateway caller task-locals；Python shell QA driver、现有 recording mock-provider fixture；不新增 crate、feature、持久化表或 harness。

**Spec:** `docs/superpowers/specs/2026-10-07-capability-phase4-h-pre-diagnostics-supplement-design.md`

**Parent Spec:** `docs/superpowers/specs/2026-10-07-capability-phase4-h-pre-runtime-mount-design.md`

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`

## Global Constraints

- 仅实现 H-pre QA control and evidence；Gate H ACP inbound、Gate I、Safe Replay、automatic replay、external-effect exactly-once、provider idempotency 和 universal scheduler 仍 deferred。
- 工具名固定为 `capability_projection_diagnostics`；只有启动环境变量精确等于 `ALEPH_CAPABILITY_DIAGNOSTICS=1` 时注册，unset 或任何其他值都不注册，且不支持 hot toggle。
- 每个操作都必须在 tool handler 内读取 `current_caller_role()`、`current_caller_is_loopback()`、`current_caller_conn_id()`；必须分别满足 `Some("operator")`、loopback 为 true、connection id 为 `Some`。不能使用 `TurnContext::caller_is_operator()` 或工具参数中的身份字段；缺失身份 fail-closed。
- `src/gateway/method_authz.rs` 的 `OPERATOR_TOOLS` 与 `src/security/dangerous_tools.rs` 的 `DANGEROUS_TOOLS` 必须包含精确工具名；allow override 或 diagnostic enablement 不绕过 handler 本身的本机授权。
- 不使用 `BUILTIN_TOOL_DEFINITIONS` 为该启动条件工具做无条件广告；沿用 runtime conditional registration，并同时保证 metadata/schema、advertisement 和真实 execute dispatch 存在。扩展既有 source census / dangerous existence guard，不添加 diagnostic-name 特例，也不通过无条件广告满足 guard。
- 所有动作只作用于已安装 production host、同一 runtime tree 和默认 production subscription；mock provider 只能记录真实 provider boundary，不能成为替代 projection consumer。
- `Cursor`、owner generation、`SessionEvent.seq` 必须保持三个独立域；bump 回包不等于已交付 generation；owner-only 变化不得改变 registry cursor。
- `hold` 只阻挡真实 `source_intake` 或 `delivery`，一次只允许一个 hold，`duration_ms` 为 `1..=5000`；close/cancel 必须 bypass hold，断线后 timer 自动释放。
- `close` 最多等待 5000 ms；超时返回 tool error，host 保持 close-requested/fail-closed，后续 status 只报告真实 non-quiescent 状态；禁止 reopen、fallback、detach join 或声称成功。
- revoke/dispose 不可逆；QA 使用 disposable profile/fixture，并在不兼容 case 之间启动新 server。`hold` 不改变 active queue capacity；较小 capacity 只能在挂载 policy 中配置。
- 已有 H-pre Tasks1–6 的正常 transport、canonical dispatch、approval、visibility/filtering、handler capture 和 completion semantics 不得旁路或弱化；不修改 `src/harness/`、session event/store、ACP、MCP approval path 或 registry authority。
- 每次 Cargo 前必须使用既有 `cargo-guard.py`：验证实际页大小、free+inactive+speculative memory 至少 `4194304` KiB、全工作树 Cargo 全局串行；不删除 `target`。
- root/package 全量测试和 clippy 的既有红灯仍然是失败；必须按精确 identity/message 记录，不得以历史数量或 baseline 名义豁免。
- English commit messages using `<scope>: <description>`；代码注释 English，文档中英双语。

## Review Focus

1. **Authorization provenance:** status 也必须拒绝缺失 role、非 loopback、缺失 connection id、spoofed argument identity；拒绝不得发生 mutation。Tests: Task 2 `diagnostics_requires_handler_local_operator_loopback_and_connection` and `spoofed_identity_arguments_do_not_authorize`.
2. **Conditional surface parity:** disabled startup must advertise nothing through builtin catalog, runtime registry, provider tools or MCP-face projections and must allocate no hold/timer machinery; enabled startup must have metadata and a real dispatch arm. Tests: Task 3 `disabled_diagnostics_has_no_advertised_or_dispatchable_surface` and `enabled_diagnostics_is_in_census`.
3. **Applied-vs-queued evidence:** status and QA receipts must inspect applied snapshot, owner observations and actual cursor, never publisher state or a log/counter. Tests: Task 1 `status_distinguishes_queued_from_applied` and Task 5 real initial/replacement receipts.
4. **Two independent recovery domains:** delivery overflow must replace stale pending events; source broadcast lag must recover from a snapshot; neither may turn owner generation into a registry cursor. Tests: Task 1 hold/recovery tests and Task 5 separate delivery/source fixture arms.
5. **Cancellation-safe close:** close during an outstanding hold must share the existing completion boundary, preserve joins after waiter cancellation/timeout, reject later reads/mutations without fallback, and not close the shared authorities. Tests: Task 1 `close_bypasses_hold_and_awaits_shared_completion` and Task 5 post-close negative arm.

## File Structure

### New files

- `src/capability/diagnostic_control.rs` — typed operation input/output, strict host-control adapter, status/hold/release/close orchestration; no gateway request parsing or identity policy outside the handler.
- `src/builtin_tools/capability_projection_diagnostics.rs` — tool-owned description/schema and execution adapter for `capability_projection_diagnostics`; delegates to the installed control adapter.
- `qa/capability_hpre/run.sh` — current-binary fixture orchestration with isolated profile, pre-redirect build, real server, stdio MCP mutation, real `chat.send`, recording provider, and deterministic cleanup.
- `qa/capability_hpre/drive.py` — JSON/RPC driver and receipt assertions for initial delivery, replacement, owner invalidation, source lag, delivery overflow, close and denial arms.

### Modified files

- `src/capability/mod.rs` — export the control adapter and expose the existing installed host/tree access needed by the tool without creating a second authority.
- `src/capability/projection_host.rs` — add the narrow diagnostic control seams: applied status snapshot, production-subscription hold gates, hold timer/release, close-request state and the existing shared completion result. Keep source/delivery/applier normal paths intact when diagnostics are disabled.
- `src/capability/ownership.rs` — expose only the existing runtime-tree mutation calls through the control adapter; no new owner authority or durable mutation log.
- `src/builtin_tools/mod.rs` and the existing builtin runtime registration/dispatch files under `src/executor/builtin_registry/` — conditionally register schema/metadata and route the exact tool name to the real adapter; use the existing runtime shape rather than `BUILTIN_TOOL_DEFINITIONS`.
- `src/executor/builtin_registry/registry/tool_registry_impl.rs` — add the real `capability_projection_diagnostics` execute arm and enforce the same enabled-state dependency as registration.
- `src/executor/builtin_registry/dispatchable.rs` and its tests — ensure the source census covers the conditional advertised and dispatchable shapes without a name-specific exception.
- `src/gateway/method_authz.rs` — add the exact name to `OPERATOR_TOOLS` and the corresponding operator hard-floor tests.
- `src/security/dangerous_tools.rs` — add the exact name to `DANGEROUS_TOOLS` and update real-name/deny tests.
- `src/builtin_tools/capability_projection_diagnostics.rs` — enforce the strict diagnostic-local identity check in the diagnostic tool handler, not through `TurnContext` or request arguments; preserve ordinary `tools.invoke` hard floors.
- `src/bin/aleph-server/commands/start/mod.rs` — conditional startup registration only for exact env value, pass the already mounted host/tree, and keep ordinary startup disabled path unchanged.
- `docs/reference/FEATURE_LOCATOR.md` — document default-off scope, destructive disposable-fixture warning, evidence boundary, and H/I deferred status.

### Explicitly do not modify

- `src/harness/` and existing QA harness semantics.
- `src/session/store.rs`, `src/session/events.rs`, `SessionEvent.seq`, ACP, Safe Replay, durable cursor, replay permit, external exactly-once or universal scheduler.
- `src/tools/registry.rs` authority, revision, handler/descriptor ownership or 256-slot broadcast contract, except a narrowly justified observation accessor proven necessary by tests.
- `src/gateway/mcp_face/mod.rs` canonical call/approval path; its notification remains notification-only.
- `src/gateway/caller_identity.rs` and `TurnContext` shape; use existing task-locals.

## Interfaces and invariants between tasks

```rust
// src/capability/diagnostic_control.rs
pub enum DiagnosticPlane { SourceIntake, Delivery }

pub struct DiagnosticStatus {
    pub lifecycle: DiagnosticLifecycle,
    pub registry_cursor: Cursor,
    pub pending_depth: usize,
    pub pending_capacity: usize,
    pub replacement_count: u64,
    pub lag_count: u64,
    pub applied_tool_ids: Vec<CapabilityId>,
    pub applied_owner_generations: HashMap<CapabilityId, OwnerGeneration>,
}

pub struct DiagnosticControl {
    // These Arcs are the already-installed host and the same runtime owner tree;
    // construction must reject any second authority rather than allocate one.
    host: Arc<ProjectionHost>,
    tree: Arc<OwnershipTree>,
}

impl DiagnosticControl {
    pub fn status(&self) -> Result<DiagnosticStatus, DiagnosticError>;
    pub fn bump_runtime(&self) -> Result<OwnerGeneration, DiagnosticError>;
    pub fn revoke_tool(&self, tool_name: &str) -> Result<(), DiagnosticError>;
    pub fn dispose_runtime(&self) -> Result<(), DiagnosticError>;
    pub async fn hold(&self, plane: DiagnosticPlane, duration: Duration) -> Result<(), DiagnosticError>;
    pub fn release(&self) -> Result<(), DiagnosticError>;
    pub async fn close(&self) -> Result<ProjectionShutdownOutcome, DiagnosticError>;
}

// src/builtin_tools/capability_projection_diagnostics.rs
pub const CAPABILITY_PROJECTION_DIAGNOSTICS: &str = "capability_projection_diagnostics";
pub async fn execute_capability_projection_diagnostics(
    request: DiagnosticRequest,
    control: Arc<DiagnosticControl>,
) -> Result<DiagnosticResponse, DiagnosticToolError>;
```

The exact serde representation may follow existing builtin tool conventions, but the operation set and safety values are fixed: `status`, `bump_runtime`, `revoke_tool { tool_name }`, `dispose_runtime`, `hold { plane, duration_ms }`, `release`, and `close`. Status must label queued/applied state; mutation responses must not claim delivery. The handler must reject the request before calling any control method unless all three ambient identity checks pass. `close` uses the host's shared completion boundary and a 5000 ms timeout without abandoning the join handles.

## Implementation Tasks

### Task 1: Add production-subscription diagnostic control primitives

**Files:**
- Modify: `src/capability/projection_host.rs`
- Create: `src/capability/diagnostic_control.rs`
- Modify: `src/capability/mod.rs`
- Test: `src/capability/projection_host.rs` and `src/capability/diagnostic_control.rs` unit tests

**Interfaces:**
- Consumes: existing `ProjectionHost::current_snapshot`, `ProjectionHost::close_and_await`, `OwnershipTree::{bump,revoke,dispose}`, `HostSnapshot`, `ProjectionShutdownOutcome`.
- Produces: `DiagnosticPlane`, `DiagnosticStatus`, `DiagnosticControl`, `DiagnosticError`, and the exact operations in the interface block above.

- [ ] **Step 1: Write failing tests for applied status and the two hold planes.**
  - `status_distinguishes_queued_from_applied` must observe `applied` only after the existing readiness/applier boundary and must expose final tool ids, cursor, owner observations, pending depth/capacity, replacement count and lag count.
  - `delivery_hold_overflow_replaces_stale_pending_state` must use a small mount-time capacity, issue more normal registry changes than capacity while delivery is held, then assert `Invalidated` precedes a replacement with the final membership.
  - `source_hold_lag_recovers_with_snapshot` must hold source intake, exceed the actual registry broadcast capacity, release, and assert lag recovery carries a snapshot/cursor rather than fabricated deltas.

- [ ] **Step 2: Run the focused capability tests and verify RED.**
  - Run the repository cargo guard, then `cargo test --package alephcore --lib capability:: --no-fail-fast`.
  - Expected: the new control types/operations or test seams are missing; do not reinterpret unrelated baseline failures as this task's result.

- [ ] **Step 3: Implement the narrow control adapter.**
  - Add a host-owned diagnostic state that gates the actual default production source intake or delivery path, permits only one `1..=5000` ms hold, rejects overlap, schedules automatic release on disconnect/timer, and lets cancellation/close bypass the gate.
  - Make `status()` read applied state and explicit queue/telemetry state, never publisher state. Keep `registry_cursor`, owner generation and session sequence separate.
  - Implement `bump_runtime`, canonical tool-id derivation plus `revoke_tool`, and `dispose_runtime` against the already installed `OwnershipTree`; mutation replies return only the authority mutation result.
  - Implement `close()` as a cancellation-safe shared wait over the existing source/applier joins with a 5000 ms timeout; timeout returns an error and preserves close-requested/fail-closed state.

- [ ] **Step 4: Run focused tests and verify GREEN.**
  - Run the guarded capability test target and the new control tests.
  - Expected: all new control tests pass; existing H-pre host tests remain green.

- [ ] **Step 5: Commit.**
  - `git add src/capability/projection_host.rs src/capability/diagnostic_control.rs src/capability/mod.rs`
  - `git commit -m "capability: add production projection diagnostic controls"`

### Task 2: Implement the tool-owned schema and strict handler authorization

**Files:**
- Create: `src/builtin_tools/capability_projection_diagnostics.rs`
- Modify: `src/builtin_tools/mod.rs`
- Test: the new tool module and handler tests

**Interfaces:**
- Consumes: `DiagnosticControl`, existing builtin definition/dispatch conventions, and `caller_identity::{current_caller_role,current_caller_is_loopback,current_caller_conn_id}`.
- Produces: exact tool name/schema and `execute_capability_projection_diagnostics(...)` dispatch adapter; the new tool handler itself is the authorization boundary for every operation.

- [ ] **Step 1: Write failing authorization and operation tests.**
  - `diagnostics_requires_handler_local_operator_loopback_and_connection` covers each missing/mismatched ambient fact for `status` and one mutating operation; assert no host/tree mutation.
  - `spoofed_identity_arguments_do_not_authorize` supplies operator/loopback/connection fields in JSON while ambient context is missing; assert denial and unchanged state.
  - `diagnostic_operations_have_bounded_schema` pins the seven operation names, `duration_ms` range and `plane` enum; no arbitrary capability kind/namespace/generation fields.
  - `close_timeout_is_reported_as_tool_error` pins that timeout is not serialized as successful closure.

- [ ] **Step 2: Run focused tests and verify RED.**
  - Use the cargo guard and the smallest relevant builtin/handler test target.
  - Expected: the tool schema/handler does not exist or does not yet enforce the strict three-part check.

- [ ] **Step 3: Implement the tool adapter and handler-local gate.**
  - Parse only the fixed operation schema, call the installed `DiagnosticControl`, and return status fields that distinguish queued from applied state.
  - In `src/builtin_tools/capability_projection_diagnostics.rs`, require `Some("operator")`, `current_caller_is_loopback() == true`, and `current_caller_conn_id().is_some()` for every operation, including `status`; reject before calling control on failure.
  - Do not read `TurnContext::caller_is_operator()` and do not accept caller identity from arguments. Keep gateway/interface code as dispatch only; host control owns behavior.

- [ ] **Step 4: Run focused tests and verify GREEN.**
  - Run guarded tests for the new tool and handler authorization.
  - Expected: every denial asserts no mutation and every enabled operation reaches the control adapter.

- [ ] **Step 5: Commit.**
  - `git add src/builtin_tools/capability_projection_diagnostics.rs src/builtin_tools/mod.rs src/gateway/handlers/tools_invoke.rs`
  - `git commit -m "security: enforce local operator diagnostics authorization"`

### Task 3: Wire conditional runtime registration, dispatch, and security census

**Files:**
- Modify: existing runtime builtin registration files under `src/executor/builtin_registry/`
- Modify: `src/executor/builtin_registry/registry/tool_registry_impl.rs`
- Modify: `src/executor/builtin_registry/dispatchable.rs` and tests
- Modify: `src/gateway/method_authz.rs` and tests
- Modify: `src/security/dangerous_tools.rs` and tests

**Interfaces:**
- Consumes: tool name/schema/execute adapter from Task 2 and startup enablement decision from Task 4.
- Produces: one conditionally advertised and dispatchable runtime tool, exact operator/dangerous hard floors, and source census coverage.

- [ ] **Step 1: Write failing disabled/enabled surface tests.**
  - `disabled_diagnostics_has_no_advertised_or_dispatchable_surface` checks catalog/runtime/provider/MCP projections and verifies no diagnostic hold/timer machinery is created.
  - `enabled_diagnostics_is_in_census` checks runtime metadata, schema, actual dispatch arm, operator list and dangerous list; deleting the dispatch arm must fail by name.
  - `diagnostics_is_not_a_gateway_surface_bypass` proves `ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics` does not bypass the handler-local gate.

- [ ] **Step 2: Run focused tests and verify RED.**
  - Use cargo guard and the relevant builtin/security test targets.
  - Expected: conditional registration and dispatch/census wiring is absent.

- [ ] **Step 3: Add the existing runtime registration shape and real execute arm.**
  - Do not add a row to unconditional `BUILTIN_TOOL_DEFINITIONS`.
  - Register metadata/schema only when the exact startup enablement is true and the installed host/control dependency exists; keep disabled startup free of diagnostic registration and timers.
  - Add the exact tool name to `OPERATOR_TOOLS` and `DANGEROUS_TOOLS`; extend the existing advertised/dispatchable source census to cover the new conditional registration shapes rather than adding a name exception.
  - Add the real `ToolRegistry::execute_tool` arm and ensure disabled state cannot reach it through ordinary catalog or direct gateway surfaces.

- [ ] **Step 4: Run focused tests and verify GREEN.**
  - Run guarded builtin, dispatchable, method-authz and dangerous-tools tests.
  - Expected: enabled metadata and dispatch agree; disabled projections contain no name; existing dangerous-name realness and census guards remain green.

- [ ] **Step 5: Commit.**
  - `git add src/executor/builtin_registry src/gateway/method_authz.rs src/security/dangerous_tools.rs`
  - `git commit -m "security: conditionally expose projection diagnostics"`

### Task 4: Install the startup-scoped control against canonical authorities

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs`
- Modify: `src/capability/mod.rs` if the installed control slot needs a typed accessor
- Test: startup source/behavior tests near existing H-pre boot tests

**Interfaces:**
- Consumes: the already mounted `ProjectionHost`, canonical `Arc<OwnershipTree>`, conditional tool registration from Tasks 1–3.
- Produces: exact-env startup enablement and a single production control target; disabled startup behavior unchanged.

- [ ] **Step 1: Write failing startup tests.**
  - `diagnostics_requires_exact_startup_env` covers unset, `0`, `true`, whitespace and `1`.
  - `diagnostics_uses_canonical_host_and_tree` proves no new registry/tree is constructed and registration happens after host mount/readiness.
  - `disabled_startup_does_not_allocate_diagnostic_hold_or_timer_state` pins behavioral equivalence of the ordinary path.

- [ ] **Step 2: Run the focused startup tests and verify RED.**
  - Use the cargo guard; do not run the full suite for this step.
  - Expected: no exact-env conditional registration/control installation exists.

- [ ] **Step 3: Wire startup conditionally.**
  - Read `ALEPH_CAPABILITY_DIAGNOSTICS` once during startup and enable only on exact `"1"`.
  - Pass the host and same `Arc<OwnershipTree>` already created at startup; do not create a second registry, host, owner tree or consumer.
  - Preserve the existing run-until-shutdown funnel and ensure the diagnostic control's close boundary does not close shared registry/ownership authorities. If the current pre-`run_until_shutdown` fallible bootstrap path cannot guarantee host drain, record that exact path as a separate unresolved lifecycle item rather than claiming this tool's close operation proves it.

- [ ] **Step 4: Run focused tests and verify GREEN.**
  - Run guarded startup/capability tests and source diff checks.
  - Expected: exact env behavior and canonical identity proofs pass.

- [ ] **Step 5: Commit.**
  - `git add src/bin/aleph-server/commands/start/mod.rs src/capability/mod.rs`
  - `git commit -m "capability: install startup-scoped projection diagnostics"`

### Task 5: Add real-binary QA and negative controls

**Files:**
- Create: `qa/capability_hpre/run.sh`
- Create: `qa/capability_hpre/drive.py`
- Modify: `qa/README.md` or the relevant bilingual QA index
- Test/evidence: `.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/task-7-report.md`

**Interfaces:**
- Consumes: current binary with Tasks1–4, existing isolated profile/bootstrap helpers, stdio MCP mutation, real `chat.send`/AgentLoop, and recording provider request body.
- Produces: receipt-level evidence for each required positive/negative arm, exit status 0 only when all required arms pass, and explicit `UNVERIFIED` status when a physical route is unavailable.

- [ ] **Step 1: Write the fixture/driver assertions before running it.**
  - Positive control: unique fixture tool appears in recording provider `tools[]` and a provider-requested invocation reaches its actual handler.
  - Normal replacement/removal: later real request changes without raw per-request registry fallback.
  - `bump_runtime`: capture old owner observations; assert applied owner values strictly increase at unchanged registry cursor and equal returned generation in isolated no-concurrent-bump fixture.
  - Owner-only `revoke_tool` and `dispose_runtime`: registry revision remains unchanged and fixture disappears from later provider requests.
  - Delivery hold: more normal changes than configured pending capacity; assert `Invalidated` then replacement and final surviving set.
  - Source-intake hold: exceed actual broadcast capacity; assert genuine lag recovery with snapshot/cursor.
  - Close during hold: close completes through shared boundary, later canonical reads refuse closed projection, no fallback, no post-close mutation delivery.
  - Denials: disabled tool, member, non-loopback, missing context and spoofed arguments fail without mutation; existing allow override does not bypass.

- [ ] **Step 2: Build the current binary before redirecting HOME and run the isolated fixture.**
  - Use the repository QA preflight and cargo guard; redirect HOME only after the current-tree binary build completes.
  - Use real `chat.send`, existing stdio MCP mutation and recording mock provider. Do not treat logs, catalog output, diagnostic counters, or a library consumer as receipts.

- [ ] **Step 3: Verify each receipt and preserve failures.**
  - Expected success: initial, replacement, owner invalidation, delivery overflow, source lag, close, and all available authorization negatives have effect-level receipts.
  - If a real non-loopback peer cannot be produced, record that physical limitation precisely and leave that arm `UNVERIFIED`; do not change exit 3 to success.
  - Ensure cleanup releases holds and terminates the disposable server even when an assertion fails.

- [ ] **Step 4: Commit QA and bilingual evidence documentation.**
  - `git add qa/capability_hpre qa/README.md .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/task-7-report.md`
  - `git commit -m "qa: prove production projection diagnostics"`

### Task 6: Document lifecycle/evidence boundaries and perform scoped validation

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/progress.md`
- Test/evidence: new Task6 report and exact Cargo/QA logs

**Interfaces:**
- Consumes: accepted implementation and QA receipts from Tasks1–5.
- Produces: bilingual operator warning, explicit H/I boundary, complete evidence index and exact baseline failure identities.

- [ ] **Step 1: Add the documentation assertions.**
  - State default-off exact env behavior, destructive disposable-profile warning, irreversible revoke/dispose/close semantics, no automatic recovery/reopen, and distinction between queued/applied, cursor/generation/seq.
  - State that mock provider is only an evidence sink and real run-loop is the consumer.
  - State any unavailable physical route or early post-mount startup path as unverified/open, never as a waived pass.

- [ ] **Step 2: Run the scoped validation matrix.**
  - Under the cargo guard: `cargo check --package alephcore --all-targets`; `cargo test --package alephcore --lib capability:: --no-fail-fast`; relevant builtin/security/startup tests.
  - Preserve exact output for later fresh validation; no target deletion or baseline substitution.

- [ ] **Step 3: Commit documentation and validation evidence.**
  - `git add docs/reference/FEATURE_LOCATOR.md .superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/progress.md`
  - `git commit -m "docs: record projection diagnostic evidence boundary"`

### Task 7: Fresh full validation and whole-branch review

**Files:**
- Test: all changed files and the final Task8 report/review package

**Interfaces:**
- Consumes: all prior commits and QA report.
- Produces: final package/root validation, independent spec+quality review, one bounded fix wave if needed, scoped rereview, and complete ruling ledger.

- [ ] **Step 1: Run fresh required package validation.**
  - Under the global cargo guard, run exactly: `cargo check --package alephcore --all-targets`; `cargo test --package alephcore --lib capability:: --no-fail-fast`; `cargo test --package alephcore --lib --no-fail-fast`; `cargo clippy --package alephcore --all-targets -- -D warnings`.
  - Record exact pass/fail counts, identities and messages; a baseline red remains red.

- [ ] **Step 2: Run fresh root validation.**
  - Under the same guard, run `cargo check --all-targets`, `cargo test --lib --no-fail-fast`, and `cargo clippy --all-targets -- -D warnings`.
  - Do not run Cargo concurrently across worktrees; do not delete `target`.

- [ ] **Step 3: Review the whole branch against both specs.**
  - Review every changed file for strict authorization, conditional surface parity, sole-authority reuse, two-domain recovery, cancellation-safe close, canonical dispatch/approval preservation, and absence of H/I/SafeReplay scope creep.
  - Reconcile every Review Focus item and every unresolved QA arm with the evidence ledger.

- [ ] **Step 4: Apply at most one focused fix wave and rerun affected tests/review.**
  - Any fix must remain within this plan; no silent baseline waiver or unrelated cleanup.
  - Preserve all prior rulings with exact cost-if-wrong notes and obtain scoped rereview of the fix.

- [ ] **Step 5: Commit final evidence only after review acceptance.**
  - `git status --short` must show no untracked product changes and no unstaged edits except explicitly preserved pre-existing documents.
  - Report the exact final commit range, QA exit status, full validation identities, unresolved blockers, and what was deliberately not done. Do not merge or push.

## Self-Review / Coverage Check

- Spec §2 authorization and registration are covered by Tasks2–4; `OPERATOR_TOOLS`, `DANGEROUS_TOOLS`, conditional runtime registration, real dispatch and census are all explicit.
- Spec §3 seven operations, one target, limits, two hold planes, 5000 ms close and no fallback are covered by Tasks1–4.
- Spec §4 real consumer, provider-boundary receipts, replacement, owner-only invalidation, both recovery domains, close and negative controls are covered by Task5.
- Spec §5 default-off allocation behavior, observational telemetry, no durable authority, destructive warning and root validation are covered by Tasks1, 3, 6 and 7.
- The parent H-pre early post-mount `?` lifecycle gap is intentionally not disguised as solved by the diagnostic tool. If completion still requires all startup error exits to drain, it must be treated as a separate approved scope decision or added to this plan by explicit written amendment before implementation.

No placeholders remain. Type names and method names used by later tasks are defined in the Interfaces block. The plan deliberately stops before implementation pending written plan approval and confirmation of the Subagent-driven execution method.
