# Panel Native Acceptance Evidence Completion Plan
# Panel Native Acceptance 证据补足计划

> Status / 状态: planning only. Do not launch WebView2 from this session.
>
> 当前状态：仅规划，本会话不启动 WebView2。

## Intent / 目标

Complete the missing evidence chain for authenticated Panel native acceptance. The previous three real WebView2 launches proved that the current shell and cleanup path can run, but all three stopped before producing usable native acceptance evidence:

- launch 1: `AMBIGUOUS`, `pairing_surface_unavailable`, incomplete exact revoke proof;
- launches 2 and 3: `BLOCKED/native_run_failed`, all six cleanup facts true, but `helperMessages`, `receipts`, and `nativeTrace` empty;
- no native `panel_credential_save/load/delete` trace, protected persistence, same-device reload, native project A/B/Global, revocation boundary, or endpoint/lease evidence.

完成 authenticated Panel native acceptance 所缺的证据链。前面三次真实 WebView2 启动已经证明 shell 和 cleanup 路径可以运行，但均未形成可用的 native acceptance 证据：

- 第一次：`AMBIGUOUS`、`pairing_surface_unavailable`，exact revoke proof 不完整；
- 第二、三次：`BLOCKED/native_run_failed`，六步 cleanup 全部成功，但 `helperMessages`、`receipts`、`nativeTrace` 均为空；
- 尚无 native `panel_credential_save/load/delete` trace、protected persistence、same-device reload、native project A/B/Global、revocation boundary 或 endpoint/lease 证据。

The next task must make a native failure explainable before spending another WebView2 launch. A generic `native_run_failed` result with empty layered evidence is insufficient for deciding whether the product path is blocked, the monitor missed a state, the helper command was never sent, the shell trace was lost, or the credential bridge was never reached.

下一任务必须先让 native failure 可解释，再消耗新的 WebView2 启动。只有 `native_run_failed` 且证据层全空，无法判断是产品路径阻塞、monitor 错过状态、helper command 未发送、shell trace 丢失，还是 credential bridge 根本未到达。

## Hard boundaries / 硬边界

- Do not modify product authentication semantics, add pairing UI, add hidden selectors, call Tauri IPC directly from the runner, inject browser storage credentials, add a fake actor, use `actor=None`, or add a test endpoint.
- Keep the existing product path: Projects/Global controls → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` → authenticated Gateway.
- Keep the existing six cleanup steps and root-last ordering. Cleanup failure or incomplete proof remains `AMBIGUOUS`.
- Keep provenance independent and exclusive-write. The runner consumes a prepared manifest and never self-pins current bytes.
- No raw token, ticket, credential bytes, lease ID, device ID, arbitrary path, or raw error text may enter reports or tracked ledgers.
- No daily daemon, port `18790`, old token-bearing connection file, wildcard credential deletion, process-name/port kill, merge or push without explicit instruction.
- Every Cargo command requires `python qa/browser_live/check_memory.py` first and `CARGO_BUILD_JOBS=1`.
- Native launch remains a separate gate. The next session must prepare and review the evidence changes before requesting a new bounded launch budget.

## Evidence model / 证据模型

Every native step must have a fixed, redacted, monotonic observation record. The report must distinguish these layers:

1. **Runner phase**: fixed phase name, outcome, fixed reason, elapsed bucket, and whether the deadline was exceeded.
2. **DOM monitor**: inspect/command name, command result, supported-surface booleans, and bounded last-known DOM state. No arbitrary selectors or page text.
3. **Helper protocol**: command class (`issue_pairing`, `confirm_pairing`, `revoke_exact`, `shutdown`, `receipt_since`), send/ack state, fixed error code, and cursor transition. No request IDs or raw payloads.
4. **Native shell trace**: fixed schema events from the spawned owned shell/helper path, including credential operation kind (`load`, `save`, `delete`), `credentialPresent`, and success/failure classification. No credential value or raw OS error.
5. **Gateway receipts**: redacted phase, sequence, revision and fixed outcome. No token, claim, device ID or lease ID.
6. **Process/provenance**: owned PID identity, manifest digest match, shell/helper/reaper digest match and exact cleanup facts.

A phase may only claim `NOT_RUN` when no mutation was attempted, `BLOCKED` when a bounded product or runtime condition refused progress, `AMBIGUOUS` when safety or evidence cannot establish the result, and `PASS` only when all required layer evidence is present.

每个 native step 都必须有固定、脱敏、单调的观察记录。报告必须区分上述六层证据。若没有证据，不得把“没有记录”解释为“没有发生”。`NOT_RUN`、`BLOCKED`、`AMBIGUOUS` 和 `PASS` 必须按固定语义使用；只有所有必要层证据齐全时才允许 `PASS`。

## Work sequence / 工作顺序

### 1. Map the current failure boundary

Read the current implementations and locate every `try`/failure return in:

- `qa/native_ipc/native_project_native_io.mjs`
- `qa/native_ipc/native_project_run_core.mjs`
- `qa/native_ipc/native_project_evidence.mjs`
- `qa/native_ipc/native_project_runner.mjs`
- `qa/native_ipc/native_project_run.mjs`
- `qa/native_ipc/real_run_io.mjs`
- `desktop/shell/src/main.rs`
- `src/bin/aleph-panel-native-qa-gateway/main.rs`
- `desktop/shell/src/keyring_bridge.rs`
- `interfaces/webchat/src/api/native_credentials.rs`

Produce a table in the plan ledger mapping each possible failure boundary to a fixed report reason. In particular, separate:

- monitor accept failure;
- inspect timeout or malformed monitor result;
- no supported pairing surface after polling;
- `ask_pair` refusal;
- pairing ticket submission failure;
- helper command not sent;
- helper ack/error timeout;
- native credential trace absent;
- credential operation failed;
- receipt timeout or stale cursor;
- project-selection command refusal;
- shell process/trace close before evidence;
- cleanup failure.

Do not launch during this step.

### 2. Add RED tests for explainable failure reports

Add deterministic failure injection to Node/VM fixtures. Each injected boundary must produce:

- the expected fixed phase;
- one fixed redacted reason;
- no generic `native_run_failed` when a more precise reason is known;
- correct evidence-layer counts;
- correct cleanup behavior;
- no raw error text or secret-bearing fields.

Add tests specifically for the current observed shape: a native run with no helper messages, no receipts and no native trace must identify the last completed boundary and report the missing layer as `native_evidence_absent` or another fixed reason, rather than silently collapsing to `native_run_failed`.

### 3. Make shell trace delivery observable

Verify the real QA shell stdout/stderr trace contract end to end. If the shell already emits trace events, test the parser against delayed, interleaved and closed-stream cases. If it does not emit the required credential operation trace, add only the QA-gated trace seam that already belongs to the shell bridge; do not alter production credential semantics.

The trace must prove effect arrival, not merely command invocation. Required fixed events are:

- `panel_credential_load` with `credentialPresent`;
- `panel_credential_save` with success classification;
- `panel_credential_delete` with success classification;
- shell child identity and bounded process status.

If the credential bridge is never reached, the report must say so explicitly and remain `BLOCKED` or `NOT_RUN` according to whether a mutation was attempted.

### 4. Make the helper/monitor boundary observable

Ensure the runner records a fixed event immediately before and after every helper command and monitor command. Keep cursor values monotonic and redact command arguments. Confirm that `helperMessages: []` means “no accepted redacted message was observed,” not “the command succeeded.”

The monitor must report the last inspect state before failure, including only the existing fixed booleans. Continue using the bounded 10-second polling behavior. Do not add a new product route or selector.

### 5. Re-run non-native validation before any launch

Run:

```text
node --test qa/native_ipc/*_test.mjs
python qa/browser_live/check_docs.py --self-test
python qa/browser_live/check_docs.py
git diff --check
```

Build current artifacts only after source changes are complete:

- Panel `panel-modular` live artifact;
- native-qa helper;
- lite QA shell with `--no-default-features --features qa-owned-root` and `TAURI_CONFIG` set to the JSON contents;
- exact QA credential reaper.

Prepare the manifest with exclusive creation and inspect only fixed digest/revision fields. Produce a read-only preflight report. No WebView2 launch is allowed if the report is not reviewable.

### 6. Request one new bounded launch budget

The next session should request or use one explicit bounded WebView2 launch only after the evidence changes and preflight are reviewable. Recommended budget: one 120-second business window. A second launch should require a new decision after reviewing the first report and should only address a localized, evidence-backed defect.

Do not spend a launch to rediscover `native_run_failed` with empty evidence.

### 7. Evaluate native acceptance by evidence, not by reachability

For the authorized launch, require this ordered proof:

1. helper and shell ownership/provenance;
2. monitor connection and supported auth surface;
3. real pairing submission or a source-backed bounded block;
4. native credential save trace;
5. same-device reload with native credential load trace and `credentialPresent:true`;
6. native Global, project A, project B and tab flow through existing UI;
7. native revocation at the next supported boundary;
8. endpoint/lease transition only if an existing product path can drive it;
9. exact six-step cleanup.

Any missing layer remains `BLOCKED`, `NOT_RUN` or `AMBIGUOUS`; do not substitute Node WS/HTTP smoke, VM tests, helper receipts, DOM observations or cleanup proof for native evidence.

## Deliverables / 交付物

- Updated runner/native evidence tests with RED/GREEN output.
- Fixed, redacted phase report for the current `native_run_failed` boundary.
- Updated bilingual run ledger and `FEATURE_LOCATOR.md` with exact evidence classification.
- Fresh current-HEAD artifact manifest and preflight report in ignored SDD evidence storage.
- At most the explicitly authorized bounded WebView2 run(s), with native trace, Gateway receipt, DOM, provenance and cleanup layers kept separate.
- Final clean worktree and conventional commit(s), without merge/push unless separately instructed.

## Completion criteria / 完成标准

The task is complete only when either:

1. all required native acceptance layers pass, including credential save/load/delete, same-device reload and project context; or
2. the task ends with a precise reproducible `BLOCKED`/`AMBIGUOUS`/`NOT_RUN` result that identifies the last reached boundary, the missing evidence layer, cleanup facts, and the exact next architectural decision.

A generic `native_run_failed` with empty helper/receipt/trace evidence is not an acceptable final explanation.
