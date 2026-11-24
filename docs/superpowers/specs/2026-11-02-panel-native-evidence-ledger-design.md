# Panel Native Evidence Ledger Design
# Panel Native Evidence Ledger 设计

- Date / 日期: 2026-11-02
- Status / 状态: Draft for approval / 待审批
- Scope / 范围: `qa/native_ipc` native authenticated project-context acceptance evidence

## Intent / 目标

The current native runner can reach the real shell and monitor, but a pre-pairing failure collapses into `native_run_failed`. When `helperMessages`, `receipts`, and `nativeTrace` are empty, the report cannot say whether the helper command was sent, whether the monitor answered, whether the shell trace channel existed, or whether the credential bridge was reached.

本设计补足失败解释链。它不改变认证、配对、项目选择、Tauri IPC 或 cleanup 语义，只让受控 native run 记录每个已经尝试和观察到的边界，并用固定脱敏值报告最后到达边界与缺失证据层。

## Goals / 目标

- Record fixed lifecycle events for every monitor and helper command.
- Record cursor transitions and bounded outcomes without request IDs, payloads, tickets, selectors, URLs, raw errors, credentials, device IDs, lease IDs, or arbitrary paths.
- Preserve `qa_trace` from the spawned Rust shell as the authoritative evidence that a credential operation completed.
- Distinguish helper readiness, command send, command response, monitor connection, monitor command result, native trace observation, receipt observation, and cleanup.
- Replace an explainable `native_run_failed` with a fixed `failureReason`, `lastBoundary`, and `missingLayers` report.
- Add deterministic RED/GREEN tests for each failure boundary, especially the observed all-empty evidence shape.
- Keep the six exact cleanup steps, root-last ordering, independent provenance manifest, one-launch gate, and all existing source/VM/Node/helper/native evidence boundaries.

## Non-goals / 非目标

- No new pairing UI, hidden selector, direct Tauri invoke, browser storage credential, fake actor, `actor=None`, or test endpoint.
- No production authentication or credential-storage behavior changes.
- No new project authority or alternate project-selection path. The accepted path remains Projects/Global → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD`.
- No claim that DOM monitor observations, helper receipts, Node WS/HTTP, VM fixtures, or cleanup prove native acceptance.
- No native shell heartbeat in the first implementation. A heartbeat is considered only if the existing trace/process diagnostics cannot distinguish a required boundary after tests.
- No new WebView2 launch until evidence changes, fresh artifacts, provenance, and reviewable preflight are complete.

## Anti-goals / 失败方式

- Do not infer success from a command being written. A command is only `sent` until its bounded response is observed.
- Do not infer that an empty evidence array means that nothing happened.
- Do not treat a page `command_ack` as proof that a native credential command ran.
- Do not emit or persist raw `Error.message`, ticket, credential, token, request ID, selector, URL, lease ID, device ID, or arbitrary command fields.
- Do not let cleanup success upgrade a blocked or ambiguous protected phase to native PASS.
- Do not let an unknown exception silently become the old generic `native_run_failed` when a fixed boundary reason is available.

## Constraints / 约束

- Runtime code direction and QA-only boundaries from `AGENTS.md` remain unchanged.
- Cargo commands, if needed, require the repository-root memory gate first and `CARGO_BUILD_JOBS=1`.
- The existing Rust trace contract in `desktop/shell/src/qa_trace.rs` emits fixed `panel_credential_load/save/delete` records after the real operation and post-operation lease check. Its `credentialPresent` field is authoritative for load; `ok` and fixed `errorCode` classify the final handler result.
- The monitor WebSocket is a DOM self-report channel. It may record command/result observations but cannot establish native IPC execution.
- The helper protocol remains closed and redacted. Cursor values are monotonic and receipt waits continue to require `seq > cursor`.
- The exact reaper remains restricted to declared `(service, account)` identities. No wildcard or process/port cleanup is introduced.

## Approaches considered / 方案比较

### A. Runner-only failure mapping / 仅 runner 失败分类

Add a fixed reason map around the current catch paths and keep the existing evidence arrays. This is the smallest change, but it cannot retain enough lifecycle information to explain whether an empty layer means “not attempted”, “sent but unanswered”, or “stream closed”. It was rejected because the observed all-empty result would remain under-specified.

### B. Cross-layer typed evidence ledger / 跨层 typed evidence ledger

Keep Rust `qa_trace` as the credential effect authority and add a closed, redacted lifecycle ledger in the JS runner/evidence layer. Each monitor/helper operation records a `before` and `after` event with a fixed operation class, phase, cursor transition where applicable, bounded result enum, and fixed reason. The final report derives `lastBoundary`, `failureReason`, and `missingLayers` from this ledger. This is the selected approach because it gives the required diagnostic precision without changing product behavior or adding a native UI route.

### C. QA-only native heartbeat / QA-only native heartbeat

Add shell startup, monitor installation, and trace-close events to the Rust QA seam. This could distinguish more process-lifecycle states, but it expands the cross-language protocol and requires extra native compilation for information that the existing child PID, monitor authentication, trace diagnostics, and helper lifecycle already partly provide. It remains a fallback only if the selected implementation proves insufficient.

## Selected design / 选定设计

### 1. Evidence ledger shape / 证据台账形状

Extend the in-memory evidence ledger with a fixed `events` collection and a derived failure summary. The exact field names may be settled by the builder, but the shape must remain closed:

```text
EvidenceEvent {
  seq: integer,
  layer: "runner" | "monitor" | "helper" | "native_trace" | "receipt" | "cleanup",
  phase: fixed phase name,
  operation: fixed operation class,
  status: "attempted" | "sent" | "observed" | "timeout" | "refused" | "closed" | "absent" | "failed",
  cursorBefore: integer | null,
  cursorAfter: integer | null,
  reason: fixed reason | null
}

FailureSummary {
  lastBoundary: fixed boundary,
  failureReason: fixed reason,
  missingLayers: fixed layer names[],
  nativeTraceStatus: "not_started" | "handle_absent" | "waiting" | "observed" | "closed" | "invalid",
  receiptStatus: "not_started" | "waiting" | "observed" | "timeout" | "invalid",
  helperStatus: "not_started" | "ready" | "command_sent" | "acknowledged" | "closed" | "failed",
  monitorStatus: "not_started" | "connected" | "command_sent" | "result_observed" | "closed" | "failed"
}
```

The public report keeps the existing `helperMessages`, `receipts`, `nativeTrace`, and `provenance` arrays for compatibility and adds the bounded ledger summary. Events are appended only through validators that reject unknown layer, operation, status, reason, or fields. Event limits follow the existing bounded window limits.

### 2. Helper command observation / helper 命令观测

Wrap `send`, `waitAck`, `waitReceipt`, `stop`, and `revokeExact` at the native runner boundary. Before writing a command, record the fixed command class and current message cursor. After an ACK, fixed error, timeout, or stream close, record the bounded result. The ledger must not include helper request IDs or command fields. `receipt_since` records `cursorBefore` and the observed receipt sequence as `cursorAfter`; `not_found` remains a retry observation, not success.

`ready` and the transient pairing ticket are recorded as lifecycle facts only after closed-shape validation. The pairing ticket remains absent from the ledger and report.

### 3. Monitor command observation / monitor 命令观测

Wrap monitor connection, `inspect`, `ask_pair`, `reveal_browser`, `submit_ticket`, reload, and project actions. Record command class before sending and the fixed `ack`/`refused`/timeout/closed result afterward. For `inspect`, store only the existing bounded DOM booleans in the last-known monitor diagnostic; no page text or selector is added. Repeated pairing-surface polls remain bounded to the current ten-second window, with each attempt represented by a fixed event count and no arbitrary response data.

A `pairing_surface_unavailable` result therefore records a connected monitor, completed inspect attempts, and no native credential attempt. It does not claim that the shell credential bridge failed.

### 4. Native trace observation / native trace 观测

Keep the current Rust `qa_trace` contract and parser as the only evidence of credential effect arrival. Add fixed lifecycle events around each trace wait and consume the existing trace diagnostics for handle absence, timeout, stream close, poison, sequence gap, and observed records. A trace record continues to contain only:

- `panel_credential_load/save/delete`;
- `ok`;
- fixed `resultShape`;
- fixed `errorCode`;
- `credentialPresent` for load;
- bounded child identity validation through the existing auth and PID checks.

If a valid trace record is observed, add it to the existing redacted `nativeTrace` array and emit `native_trace observed`. If the wait expires without a record, emit `native_trace absent` with a fixed reason such as `native_trace_timeout`; if the stream is unavailable or poisoned, use the corresponding fixed reason. No Rust change is planned unless the implementation audit shows that an existing parser cannot expose this bounded status.

### 5. Receipt observation / receipt 观测

Wrap every receipt wait with fixed lifecycle events. Preserve the current `seq > cursor` rule and same-cursor retry for uncorrelated `not_found`. Record only redacted receipt fields already accepted by `validateHelperMessage`: operation, fixed project label, fixed result/status, revision shape, and sequence. A receipt timeout reports the last cursor and missing receipt layer, never a raw helper response.

### 6. Failure derivation / 失败推导

Replace positional `PHASES[state.phases.length]` as the sole failure classifier with a boundary-aware mapper. The mapper consumes the validated ledger and the thrown fixed error code, then selects the most specific reason. It must be deterministic and fail closed:

- preflight/artifact failures map to their existing fixed provenance reasons;
- helper spawn/ready/send/ack/close failures map to fixed helper reasons;
- monitor accept/auth/send/result/close failures map to fixed monitor reasons;
- unsupported or absent pairing surface maps to `pairing_surface_unavailable`;
- trace handle/timeout/close/poison/credential operation failures map to fixed native-trace reasons;
- receipt cursor/timeout/invalid failures map to fixed receipt reasons;
- cleanup failures remain `cleanup_incomplete`; budget exhaustion remains `window_budget_exceeded` and can make the overall run `AMBIGUOUS`;
- an unrecognized internal branch maps to a fixed `internal_unclassified` reason together with the last validated boundary, never raw exception text.

The all-empty observed shape must produce a report such as: monitor connected and inspect completed; last boundary `monitor_result`; failure reason `pairing_surface_unavailable`; missing layers `native_trace`, `receipt`, and protected credential effect. The exact labels will be closed in the implementation and tests, but generic `native_run_failed` is not an acceptable mapped reason when a more specific boundary exists.

### 7. RED/GREEN tests / RED/GREEN 测试

Add deterministic tests beside the existing `qa/native_ipc` tests for:

- each helper send/ack/error/timeout/close boundary;
- each monitor connection/command/refusal/timeout boundary;
- no pairing surface after bounded polling;
- absent, delayed, closed, poisoned, and successful native trace records;
- receipt timeout, stale cursor, retry, and success;
- all-empty helper/receipt/trace evidence with a precise last boundary and missing layers;
- no raw secret-bearing or arbitrary fields in events and summaries;
- cleanup execution and outcome semantics remaining unchanged;
- public report compatibility and fixed reason validation.

Each test must first express the missing behavior as RED, then pass after the implementation. Tests remain Node/VM/helper-level unless the trace contract itself changes; no native launch is used as a unit test.

### 8. Documentation and launch gate / 文档与启动闸门

After implementation and validation, update the bilingual run ledger and `docs/reference/FEATURE_LOCATOR.md` §6.11.2 with the exact evidence classification and the current fixed failure boundary. Generate fresh current-HEAD artifacts and the independent provenance manifest only after source changes are complete. Produce a reviewable preflight report. A new WebView2 launch remains a separate explicit authorization gate and is limited to the approved business window; no launch is part of this design approval.

## Reference project lessons / 参考项目经验

A read-only comparison with `T:/Github/deepseek-harness/apps/desktop/publishing/README.md` supports three choices in this design. The reference retains a separate plan, newline-delimited lifecycle events, and a terminal result; a missing terminal result means interruption or an unknown outcome, never success. It records intent before the external operation and available response facts afterward, keeps raw credentials and raw SDK errors out of the record, stops dependent writes when audit persistence fails, and requires remote reconciliation after a timeout or missing receipt because the external effect may already have happened.

Aleph applies the same discipline within its bounded native run: record command intent before sending, record the bounded response separately, retain partial in-memory evidence after failure, never interpret an absent receipt as proof of absence, and keep native effect evidence separate from the runner's own observations. The comparison is a design reference only; no DeepSeek Harness code, dependency, or file format is imported.


The builder may choose the exact module name for the closed event validator, the exact event field casing, and the internal helper functions, provided the public report semantics, fixed reason vocabulary, redaction rules, and evidence authority above remain unchanged. The builder may reuse existing diagnostics instead of adding a Rust event when they expose the required status. The builder may settle test fixture construction and event ordering details that do not change the boundary classification.

## Failure and missing-layer decisions / 失败与缺失层决策

The first business failure is retained as `primaryFailure` with its boundary, fixed reason, and operation context. Cleanup has an independent `cleanupOutcome` and `cleanupReason`; cleanup never overwrites the business root cause. The overall outcome still follows the existing state rules, so incomplete cleanup remains `AMBIGUOUS` even when the protected phase already has a more specific blocked reason, and budget exhaustion remains `AMBIGUOUS`.

`missingLayers` is calculated for the current failed phase and current fixed operation, not as a single run-wide set. Earlier evidence cannot satisfy a later operation: an observed save trace does not satisfy a missing post-reload load trace. The report may retain the existing aggregate evidence arrays, but the failure summary is operation-scoped.

The builder check initially timed out without output and was resumed successfully. It raised only the two questions above; both decisions are now recorded here. No other material planning question remains.


- No explainable native failure is reported only as `native_run_failed`.
- The current empty-evidence shape identifies the last reached boundary and missing evidence layers.
- Every monitor/helper operation has bounded send/result lifecycle evidence with fixed command class and cursor data where relevant.
- Rust credential trace remains effect-after-operation evidence and distinguishes load/save/delete, success/failure, and `credentialPresent` without secrets.
- RED/GREEN tests cover all planned boundaries and reject raw/unbounded fields.
- Existing cleanup order, provenance independence, evidence-layer separation, and launch gate remain intact.
- The design does not authorize implementation, artifact builds, or WebView2 launch until approved.

## Open items for approval / 待审批问题

- Approve the selected cross-layer ledger and the closed report shape above.
- Confirm that the implementation may update the runner/evidence tests and bilingual evidence documentation, while keeping product authentication semantics unchanged.
- Confirm that a Rust QA-only change is allowed only if the existing trace diagnostics cannot expose a required lifecycle state; otherwise the implementation remains JS/Node-only.
