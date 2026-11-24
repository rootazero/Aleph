# Restart prompt: complete Panel native authenticated project-context acceptance

你是 Codex，继续在 `D:/Workspace/Aleph` 的同一个 Aleph 仓库中直接完成 Panel authenticated project-context native acceptance。不要使用子代理，不要启动任何 CLI 模型。你必须持续推进到完成，或到达真实且可复现的阻塞；不要以 generic `native_run_failed` 收尾。

## 当前仓库状态

- 当前分支应为本地 `main`。
- selector 产品修复已经在 `main`：`bb1bd7cf803e7b3b17b5cb6f717523ef3dba5a0b`，提交标题 `fix(panel): make exec tier selection failures visible`。
- 不要回退、重写或 reset 历史。
- 现有 native QA/docs 改动可能仍在工作区，必须先 `git status --short` 盘点；只修改本任务允许的 native QA、测试、计划、run ledger 和 `FEATURE_LOCATOR.md` 文件，不要把既有未提交改动误删或重置。
- selector 任务已经独立完成，不要借 native 任务重做 selector，也不要把 selector 修复当作 native 证据。
- 计划文件：`docs/superpowers/plans/2026-11-03-panel-native-acceptance-completion.md`。
- 旧证据账本：`docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`。
- 最新 native run：`d044d278ae098998cb40c8c62166e23124be4f219a3b7d785f761a9fecccbfa9`，到达 `helper_ready PASS`、`protected_pairing PASS`，接受 save trace + helper receipt，随后在 `protected_reload / monitor_command / monitor_auth_failed` BLOCKED；六项 cleanup 全部 true。
- 旧 native provenance 在 selector commit 后已过期。必须针对当前 HEAD 重新构建 artifact 并独立生成 provenance，不能复用旧 manifest。

## 最高优先级硬约束

1. 不修改产品认证语义，不新增 pairing UI、hidden selector、direct Tauri invoke、browser storage credential、fake actor、`actor=None`、test endpoint、产品 RPC 或 wire field。
2. 复用真实产品路径：Projects/Global → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` → production authenticated Gateway。
3. credential effect 只能由 owned shell 的 authenticated `ALEPH_QA_TRACE` 证明。Node WS/HTTP、VM、DOM、helper receipt、monitor activity、cleanup 和 provenance 必须单独标注，不能互相升级。
4. 保持六步 exact cleanup 和 root-last 顺序：`shell_stopped`、`helper_stopped`、`exact_devices_revoked`、`credential_deleted_exact`、`credential_absent_exact`、`owned_root_removed`。
5. 只允许 exact `(service, account)` credential reaper；不 wildcard 删除，不按进程名或端口 kill。
6. 不连接、停止或替换日常 daemon，不碰 `18790`，不读取旧 token-bearing connection file。
7. 每个 Cargo 命令前，必须从仓库根运行 `python qa/browser_live/check_memory.py`，并使用 `CARGO_BUILD_JOBS=1`。不要用 `bash -lc` 隐藏环境设置。
8. 不要在修复和 preflight 完成前启动 WebView2。新的 launch 是独立 gate；只有 reviewable preflight 后才请求用户明确授权。
9. 每次明确授权最多一个新的 120 秒 business window。第一窗口结束后停止；第二窗口必须基于第一份报告再次获得明确授权。
10. 没有新的 launch 授权时，不得执行 `node qa/native_ipc/native_project_runner.mjs --execute`。

## 已知必须先解决的具体问题

`qa/native_ipc/real_run_io.mjs::createMonitorServer` 规定：已有 WebView2 document 连接时，只有先调用 `expectReload()` 才接受第二个 document。参考实现 `qa/native_ipc/real_run_core.mjs` 在发送 reload 前调用了 `deps.monitor.expectReload()`；但当前 `qa/native_ipc/native_project_native_io.mjs` 的 reload 路径只发送 `monitorCommand("reload", ...)`，没有先 arm reload。这是下次 launch 前必须修复和测试的具体 harness defect。

正确顺序必须是：

1. `currentPhase = "protected_reload"`；
2. 调用已有 monitor dependency surface 的 `monitor.expectReload()`；
3. 记录固定、脱敏的 reload-arm lifecycle event；
4. 在旧 authenticated connection 上发送现有 `reload` command；
5. 接受并验证新 document 的 authenticated hello；
6. 才等待 post-reload `panel_credential_load` trace。

不要新增 command，不要新增产品 route，也不要把 `expectReload()` 放到 reload command 之后。

## 必须先阅读的文件

完整阅读并记录每个 failure boundary：

- `AGENTS.md`
- `docs/reference/WINDOWS_RUNTIME.md`
- `docs/reference/DESKTOP_SHELL.md`
- `docs/reference/DESKTOP_BRIDGE.md`
- `docs/reference/FEATURE_LOCATOR.md` §6.11.2
- `docs/superpowers/plans/2026-11-02-panel-native-acceptance-evidence-completion.md`
- `docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`
- `qa/native_ipc/native_project_native_io.mjs`
- `qa/native_ipc/native_project_run_core.mjs`
- `qa/native_ipc/native_project_evidence.mjs`
- `qa/native_ipc/native_project_runner.mjs`
- `qa/native_ipc/native_project_run.mjs`
- `qa/native_ipc/real_run_io.mjs`
- `qa/native_ipc/real_run_core.mjs`
- `qa/native_ipc/monitor_contract.mjs`
- `desktop/shell/src/main.rs`
- `desktop/shell/src/qa_trace.rs`
- `desktop/shell/src/keyring_bridge.rs`
- `src/bin/aleph-panel-native-qa-gateway/main.rs`
- `interfaces/webchat/src/api/native_credentials.rs`

把源码事实写入计划目录下的 progress/ledger 文件，不要凭记忆跳过审计。

## Failure boundary 目标

为以下每个 boundary 建立固定 reason、最后到达 boundary 和缺失层映射：

- artifact/provenance missing, invalid, stale, hash mismatch;
- owned root create/write/ownership failure;
- helper ready timeout/closed/invalid;
- monitor channel refusal、origin refusal、stream invalid、hello timeout、hello auth failure、connection close;
- reload not armed、reload command refusal、new-document auth failure;
- inspect timeout、inspect invalid、pairing surface unavailable;
- `ask_pair` refusal、ticket missing/submission failure;
- helper command not sent、ack timeout、helper protocol error;
- native trace absent、closed、poisoned、invalid ingestion、credential operation failed;
- receipt timeout、stale cursor、uncorrelated `not_found`、invalid receipt;
- project Global/A/B command refusal or receipt absence;
- endpoint/lease unsupported boundary;
- exact revoke ambiguity/failure;
- shell/helper stop, exact credential delete/absence, and root-last cleanup failure.

固定 reason 必须闭集、脱敏、可测试；unknown/raw error 必须映射为 `internal_unclassified`，但已存在的安全 public reason 要保持兼容。报告不能落 raw URL、Origin、port、path、ticket、token、credential、request ID、device ID、lease ID、命令 payload 或 raw OS error。

## 实现顺序

### 1. 先写 RED，再修 reload arming

在 `qa/native_ipc/native_project_native_io_test.mjs` 或合适的现有 native test 文件中增加测试：

- fake monitor 在 reload command 前必须收到 `expectReload()`；顺序错误则测试失败；
- unarmed second WebView2 connection 被拒绝；armed reload connection 被接受；
- reload monitor failure 的 summary 保留 `primaryFailure.operation`、`lastBoundary`、`failureReason`，不能复用 pairing evidence；
- monitor auth failure、concurrent refusal、no hello timeout 分别得到不同固定 reason。

先运行测试确认 RED，再做最小实现。实现 `monitor.expectReload()` dependency surface（如果尚未存在），并在 native runner reload 路径中于 command 前调用它。

### 2. 增加固定 monitor diagnostics

在 `createMonitorServer` 已有 counters 的基础上，给 monitor dependency 增加只读 fixed-shape snapshot。允许字段仅包括有界 counters 和闭集 enum：`upgrades`、`accepted`、`helloSeen`、`refusedOrigin`、`refusedOriginKinds`、`refusedOther`、`refusedConcurrent`、`streamInvalid`、`installFailed`、`installFailedReason`。

将该 snapshot 作为 evidence/failure report 的固定字段或固定 diagnostic event。禁止输出原始 Origin、URL、host、port、socket address、token、nonce、WebSocket error 或任意字符串。

定义确定性的 reason priority，至少区分：

- reload 未 arm 且出现 concurrent refusal → `reload_not_armed`；
- origin-only refusal → `monitor_origin_refused`；
- other channel refusal → `monitor_channel_refused`；
- accepted socket 但 first stream invalid → `monitor_stream_invalid`；
- deadline 前无 accepted/hello → `monitor_connect_timeout`；
- hello 到达但 auth/origin 不匹配 → `monitor_auth_failed`；
- accepted connection closed before required response → `monitor_connection_closed`。

### 3. 审计并补齐 trace/helper/receipt lifecycle

保持现有 evidence ledger 契约：

- `helperMessages: []` 只表示没有 accepted redacted helper message；
- `receipts: []` 只表示 cursor 后没有 accepted receipt；
- `nativeTrace: []` 只表示没有 accepted selected credential effect；
- wire trace sequence 在 `NativeTraceStream` 内必须连续；selected ledger 只拒绝重复/回退 seq；
- `primaryFailure` first-failure-wins；cleanup 独立；
- `missingLayers` 按失败 phase + operation 计算。

确保 helper send/ack/stop/revoke、monitor command/inspect/reload-arm、trace attempted/observed/absent/closed/failed、receipt sent/observed/timeout/retry 都有闭集固定事件。

验证 `desktop/shell/src/qa_trace.rs` 的 save/load/delete 固定 shape：save 成功为 `resultShape="null"`，load 成功为 `resultShape="lease_object"` 且有 `credentialPresent`，delete 使用固定 success/error shape。除非测试证明 parser/QA trace defect，否则不改 Rust credential semantics。

### 4. 非 native RED/GREEN

从仓库根运行：

```text
node --test qa/native_ipc/*_test.mjs
python qa/browser_live/check_docs.py --self-test
python qa/browser_live/check_docs.py
git diff --check
```

然后按需运行 Cargo；每次前先 memory gate，并设置 `CARGO_BUILD_JOBS=1`：

```text
cd interfaces/webchat && CARGO_BUILD_JOBS=1 cargo test --lib --no-default-features --features panel-modular
cd interfaces/webchat && CARGO_BUILD_JOBS=1 cargo check --lib --no-default-features --features panel-modular
cd shared/ui_logic && CARGO_BUILD_JOBS=1 cargo test --lib composer_dials
```

根目录 `bun run check` 当前没有 `check` script；可以尝试一次，但必须如实记录 `error: Script not found "check"`，不能把它称为通过。完整 workspace fmt/clippy 如果被基线格式阻塞，也必须记录 exact failure，不要修复本任务外的 baseline。

Node gateway smoke 必须从仓库根运行；从 `qa/native_ipc` 运行会产生相对 `target/debug` 路径 ENOENT 假失败。

### 5. Fresh artifact/provenance

所有 source changes 和 non-native tests 通过后：

1. 从 `D:/Workspace/Aleph/interfaces/webchat` 构建：
   `bun run panel:tabs:build -- --clean --output=.panel-live/shell`
2. 按 memory gate + `CARGO_BUILD_JOBS=1` 构建当前 HEAD 的 lite shell、native-qa gateway、exact credential reaper。
3. 检查 `build.json.revision === git rev-parse HEAD`。
4. 检查 descriptor entry、host/terminal JS/WASM、HTML、CSS、manifest。
5. 对当前 HEAD 独立 exclusive 生成 `interfaces/webchat/.panel-live/native-provenance.json`；旧 manifest 只能在确认是 ignored stale generated file 后删除，不能覆盖写入。
6. 独立 inspect provenance，仅输出固定 revision/digest/contract 字段。
7. 保存 build/provenance/audit/preflight 到 ignored `.superpowers/sdd/`，不把秘密或原始路径写进 tracked docs。

### 6. Reviewable preflight

运行：

```text
node qa/native_ipc/native_project_runner.mjs
```

必须得到：

- `outcome: "NOT_RUN"`;
- `reason: "preflight_only"`;
- `webviewLaunches: 0`;
- `launchGate: "new_explicit_budget_required"`;
- `artifacts.ok: true`;
- exact six cleanup sequence unchanged;
- no native evidence falsely claimed。

如果 artifacts/provenance 不通过，停止修复，不能启动 WebView2。

把 preflight、diff、测试摘要和当前 failure-boundary table 作为 reviewable concrete result，再向用户请求一个新的 120 秒 business window。计划 approval、selector commit、旧 native window 授权都不能替代这次授权。

### 7. One bounded native launch

只有用户明确授权后执行一次：

```text
node qa/native_ipc/native_project_runner.mjs --execute
```

不要同时运行任何 ad-hoc helper/browser/reaper 命令。严格按 runner 的 ownership 和 cleanup path 执行。

必须按以下顺序收集 evidence：

1. provenance/artifact/owned-root identity;
2. helper ready;
3. authenticated monitor hello;
4. bounded pairing-surface inspect;
5. real pairing submission;
6. accepted native save trace;
7. helper confirmation and receipt;
8. `monitor.expectReload()` armed before reload;
9. authenticated new document;
10. accepted native load trace with `credentialPresent:true`;
11. reload receipt;
12. Global discovery;
13. project A;
14. project B same revision and old-A fencing;
15. supported endpoint/lease boundary, or explicit fixed BLOCKED reason if current product path cannot drive it;
16. revocation boundary;
17. exact six cleanup facts.

如果第一个窗口仍 BLOCKED，不自动第二次启动。先根据报告决定是否需要新的代码修复、架构决策或新的明确授权。

## Final report requirements

最终中文回复必须列出：

- 完成的代码、测试、artifact 和文档；
- 实际执行的命令及结果；
- native run ID、phase outcomes、accepted `helperMessages`/`receipts`/`nativeTrace`；
- `primaryFailure.phase`、`primaryFailure.operation`、`lastBoundary`、`failureReason`、`missingLayers`；
- monitor diagnostics、trace diagnostics、helper/receipt cursor diagnostics；
- 六项 exact cleanup facts；
- 未证明的 persistence/reload/project/lease/revocation 层；
- review、format、hook、command 和环境限制；
- 是否需要新的 architecture decision 或 launch authorization。

Generic `native_run_failed` 且 helper/receipt/trace 全空不是可接受结论。
