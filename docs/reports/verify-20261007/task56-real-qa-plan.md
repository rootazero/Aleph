# Task #56 real-QA preparation: fail-closed preflight + monitor contract / Task #56 真实 QA 准备：fail-closed 预检 + 监控契约

Status / 状态：**PREPARATION ONLY — #56 IS NOT COMPLETE.** No real run happened. Nothing here launched a shell, Gateway, browser/WebView2, Credential Manager command, or any process other than `node --check` / `node ... --self-test` / read-only `node ... <normal mode>` refusals. Overall verdict unchanged: **PARTIAL, not production ready**. / **仅为准备工作，#56 未完成。** 没有任何真实运行：未启动 shell、Gateway、浏览器/WebView2、凭据管理器命令或其他进程，仅运行了 `node --check`、`--self-test` 与只读的普通模式拒绝。整体结论不变：**PARTIAL，非 production ready**。

## Files / 文件

| File / 文件 | Role / 作用 |
|---|---|
| `qa/native_ipc/real_qa_prep.mjs` | Preflight/planning harness. Pure auditable helpers + `--self-test` + normal mode that validates then **always** refuses `NOT_RUN` (exit 4). Node stdlib only (`node:crypto/fs/path/url`); contains no process-launch, network, env, HOME or registry code (self-audited). / 预检与规划 harness：可审计纯函数 + `--self-test` + 普通模式（校验后**一律**拒绝，`NOT_RUN`，退出码 4）。仅 Node 标准库，不含任何进程启动 / 网络 / 环境变量 / HOME / 注册表代码（自检）。 |
| `qa/native_ipc/monitor_contract.mjs` | Pure design module: future same-origin WebSocket monitor event schema + strict `validateEvent` / `StreamValidator` / `validateMonitorEndpoint`. No production code emits these events yet. / 纯设计模块：未来同源 WebSocket 监控事件 schema 与严格校验器；目前没有任何生产代码发出这些事件。 |
| `qa/native_ipc/run.mjs` | **Unchanged** (memory-backend fake-page harness). / **未改动**。 |

No Rust, Cargo, source, `docs/reference/*`, or `qa/README.md` edits were made by this task. / 本任务未改动 Rust、Cargo、源码、`docs/reference/*`、`qa/README.md`。

## Exit codes / 退出码

`0` self-test ok · `1` self-test failed · `2` invalid input · `3` ambiguous cleanup (reserved for the future runner, see `finalExitCode`) · `4` NOT_RUN refusal. / 0 自检通过 · 1 自检失败 · 2 输入非法 · 3 清理含糊（保留给未来 runner，见 `finalExitCode`）· 4 NOT_RUN 拒绝。

## Helpers and what they enforce / 辅助函数与约束

- **Nonce / service**: `validateNonce` = exactly 32..128 lowercase hex; `validateService` = exactly `aleph-qa.device-credential.` + such a nonce (production `aleph-client.device-credential` and prefix-only rejected). Mirrors `desktop/shell/src/qa_root.rs`. `freshNonce()` = 16 random bytes. / nonce 32..128 位小写 hex；service 必须恰为 `aleph-qa.device-credential.<nonce>`。
- **Credential target identity**: `credentialTarget` yields exactly `{service, account}`; `account` mirrors `shared/client` `account_for_gateway` (`scheme://host:port`, lowercase, loopback only here). `assertExactTarget` rejects extra fields, wildcard/whitespace/control chars. `credentialOp` allows only `delete_exact` / `verify_absent_exact`; list / prefix / delete-all are unrepresentable. Logs carry only `targetDigest` (sha256 of service+NUL+account). **The Windows target-name string derived by the keyring crate is deliberately NOT modeled** (unverified); the future delete tool must operate on (service, account) through the same keyring API, not on a guessed target string. / 目标身份仅 `{service, account}`；日志只含摘要；不模拟 keyring crate 的 Windows target 名（未验证），未来删除工具须通过同一 keyring API 按 (service, account) 精确删除。
- **Events** (`makeEvent` / `validateHarnessEvent`): fixed event names (`plan_created`, `refusal`, `pid_recorded`, `window`, `cleanup`), exact key sets, per-field validators, secret-like key names rejected; values/secrets cannot be expressed (no free text; `reason` is one fixed constant). / 固定事件名与键集，无自由文本，拒绝类密钥字段名。
- **Window**: integer 1000..180000 ms; `validateWindowBudget` allows window index 1..3 only (historical; `MAX_WINDOWS` was later raised to 4 by explicit user approval for one extra window, see `task56-native-real-window-4.md`; a final fifth window, `MAX_WINDOWS=5`, was later user-approved and failed at `reveal_browser`, see `task56-native-real-window-5.md`; budget now exhausted) and requires the prior windows (`--prior-window-ms a,b`) to match. / 单窗口 ≤180000 ms，最多 3 个窗口。
- **PID ownership record**: only `role:"shell"`, `provenance:"harness_spawned"`, absolute paths, `aleph-shell-qa-*` root, valid nonce. `mayTerminate` additionally requires the caller-observed live command line to contain **both** root and nonce, pid > 4 and ≠ own pid. Gateway/external/adopted processes can never satisfy it. / 仅 harness 自己启动的 shell 可被终止，且需命令行同时含 root 与 nonce。
- **Cleanup state machine**: `NOT_ARMED → ROOT_CREATED → NAMESPACE_ARMED → PROCESS_STARTED → PROCESS_STOPPED_VERIFIED → CREDENTIAL_ABSENT_VERIFIED → CLEAN`. Any unknown state/event or out-of-order step absorbs into `AMBIGUOUS`; `finalExitCode` = 0 only for `CLEAN`/`NOT_ARMED`, otherwise **3**. `evaluateCleanup` requires each observation to be literally `true` (`false`, `"unknown"`, missing → 3). / 任何越序或未知转移吸收到 AMBIGUOUS，退出码 3。
- **Plan + audit** (`buildPlan` / `auditPlan`): declarative data only (`executed:false`); fixed 9-step order; launch argv exactly `<exe> --qa-root <OWNED_ROOT> --qa-nonce <nonce> --qa-credential-backend os`, env `inherit_unchanged` (no `HOME`/`USERPROFILE` override); credential steps exactly equal the target; string scan rejects credential-CLI, registry (`HKCU`), `HOME`/`USERPROFILE`/AppData, and any stop/restart/kill wording tied to port 18790; the only stop step targets a harness-spawned PID record. 18790 may appear only as the (read-only) Gateway the page connects to. / 计划只是数据；argv 固定；不覆盖 HOME；凭据步骤精确；任何针对 18790 的 stop/restart/kill 被拒绝。
- **Readiness gate**: `PREREQUISITES` = `monitorInjectionSeam:false`, `realGatewayLaunchSupported:false`, `exactCredentialDeleteTool:false`. Normal mode has no launch code at all, so it refuses unconditionally with `NOT_READY_NO_MONITOR_OR_REAL_GATEWAY`. / 普通模式没有任何启动代码，无条件拒绝。
- **Exe check** (normal mode, read-only): absolute `.exe`, regular file (not symlink), contains `aleph-qa-owned-root-build-v1`. Note: this marker proves a QA-owned-root build, **not** that the `os` backend is compiled in. / 只读检查 exe；该 marker 只能证明 QA 构建，不能证明含 `os` 后端。

## Monitor contract (future, same-origin WebSocket) / 监控契约

Invariants M1–M6 (see header of `monitor_contract.mjs`): observe-only (SUPERSEDED: the page-side invoke wrapper/`invoke_end` is removed; the same fixed shape is now the native stdout trace record `ALEPH_QA_TRACE`, see `NativeTraceStream`; no field can carry an invoke argument/result value; it = ok flag + fixed result-shape enum or fixed native error-code enum + `credentialPresent` boolean for load only); fixed command names (`panel_credential_load/save/delete`), exact argument **key names** per command; connect evidence = redacted `{method, sorted paramKeys}` whose digest the validator **recomputes** (so it cannot be a hash of a secret); DOM commands are a fixed 4-value enum (no selectors/text); `hello` first, run token on every event, contiguous `seq`, strictly serial invoke start→end; run token must differ from the credential nonce; endpoint same-origin, `ws`/`wss` matching `http`/`https`, fixed path `/__qa_monitor/<run>`, no userinfo/query/fragment; per-event ≤512 bytes. / 监控只观察，不能携带或伪造 invoke 结果；连接证据仅为脱敏形状 + 可重算摘要；DOM 命令固定枚举；同源。

Caveat: `CONNECT_PARAM_KEY_NAMES` is a **placeholder** set that must be reconciled with the real Gateway connect schema before implementation. The error-code enum mirrors `BridgeError` (`desktop/shell/src/keyring_bridge.rs`, serde snake_case) and must be kept in sync. / 注意：连接参数键名集合是占位，需对齐真实 Gateway 协议；错误码枚举镜像 `BridgeError`，需同步维护。

## Actual verification run / 实际执行的验证

Commands (cwd = worktree; no Cargo, no other process):

```
node --check qa/native_ipc/real_qa_prep.mjs   -> ok
node --check qa/native_ipc/monitor_contract.mjs -> ok
node qa/native_ipc/real_qa_prep.mjs --self-test
  {"mode":"self-test","total":30,"passed":30,"failed":[],"noExternalSideEffects":true}   exit=0
```

Normal mode (read-only exe scan of `target/debug/aleph-desktop-shell.exe`; nothing launched):

| Input / 输入 | Result / 结果 |
|---|---|
| valid args, `--window-ms 180000 --window-index 1`, `ws://127.0.0.1:18790/ws` | `status:"NOT_RUN"`, `reason:"NOT_READY_NO_MONITOR_OR_REAL_GATEWAY"`, `launched:false`, `unmetPrerequisites:["monitorInjectionSeam","realGatewayLaunchSupported","exactCredentialDeleteTool"]`, **exit 4** (the exe contained the QA marker; plan printed as data, a throw-away random nonce) |
| `--window-ms 180001` | `INVALID_INPUT window_exceeds_180000`, exit 2 |
| no args | `INVALID_INPUT required:--exe`, exit 2 |
| `ws://example.com:1/ws` | `INVALID_INPUT url_not_loopback`, exit 2 |
| `--window-index 4` | `INVALID_INPUT window_index_out_of_range`, exit 2 |

Mutation sanity (temp copy in `/tmp`, since removed): raising `MAX_WINDOW_MS` to 999000 made the self-test fail (26/30), so the window checks are not vacuous. / 变异检查：放宽窗口上限后自检失败（26/30），说明检查非恒真。

The 30 self-tests cover: nonce/service, account derivation, exact target + wildcard refusal, credential ops, window + budget, PID record + ownership proof, cleanup machine (happy + every ambiguity → 3), event schema, plan audit mutations (stop/restart gateway, wildcard credential, env/HOME/registry/credential-CLI strings, argv tampering, reordering), argv/run validation, readiness gate, marker scan across chunk boundaries, monitor event/stream/endpoint rules, normal-mode refusals, and a source audit (stdlib-only imports; no child-process / env / network / homedir primitives in either file).

## NOT done — this does not complete #56 / 未完成项（本工作不代表 #56 完成）

- No real Native run: no visible window, no Gateway, no OS credential entry created or deleted, no WebView2.
- No monitor implementation or injection seam exists; the contract is unexercised against any real emitter.
- No exact credential delete + absence-verify tool exists (`exactCredentialDeleteTool:false`); the Windows credential target string and the delete API behavior are unverified. Running `--qa-credential-backend os` before that tool exists would leave residue.
- Cleanup state machine and PID ownership helpers are pure logic; no process enumeration / termination / root removal is implemented in this file.
- The exe QA marker does not prove the `os` backend is present; no Rust-side check of `qa/native_ipc` expectations beyond the earlier compiled seam.
- `run.mjs` memory harness not re-run; `qa/README.md` / `docs/reference/*` not updated. Placeholder connect key names unreconciled. Pre-existing unrelated modifications in the worktree were not inspected or touched.
- The 3-window / 180 s authorization is encoded as validation only; no window was consumed.
